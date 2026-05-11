// 模块解析器：文件系统模块解析、import/export 提取

use anyhow::{Context, Result, bail};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use swc_core::common::Span;
use swc_core::ecma::ast;

pub use wjsm_ir::ModuleId;

/// 解析后的模块信息
#[derive(Debug)]
pub struct ResolvedModule {
    pub id: ModuleId,
    pub source: String,
    pub path: PathBuf,
    pub ast: ast::Module,
    pub imports: Vec<ImportEntry>,
    pub exports: Vec<ExportEntry>,
    /// 动态 import() 的 specifier 列表（不合并进 imports）
    pub dynamic_imports: Vec<String>,
    pub is_cjs: bool,
}

/// Import 声明条目
#[derive(Debug, Clone)]
pub struct ImportEntry {
    /// 模块说明符（如 './foo'）
    pub specifier: String,
    /// 导入的名称列表：(local_name, imported_name)
    /// - `import { x } from './foo'` → ("x", "x")
    /// - `import { y as z } from './foo'` → ("z", "y")
    /// - `import * as ns from './foo'` → ("ns", "*")
    /// - `import defaultExport from './foo'` → ("defaultExport", "default")
    pub names: Vec<(String, String)>,
    pub source_span: Span,
}

/// Export 声明条目
#[derive(Debug, Clone)]
pub enum ExportEntry {
    /// export { x } / export { x as y }
    Named { local: String, exported: String },
    /// export default expr
    Default {
        local: String, // 内部生成的变量名
    },
    /// export * from './foo'
    All { source: String },
    /// export { x, y } from './foo' — 命名重导出
    NamedReExport {
        local: String,
        exported: String,
        source: String,
    },
    /// export const/let/var/function/class
    Declaration { name: String },
}

/// 模块解析器
pub struct ModuleResolver {
    root_path: PathBuf,
    next_id: u32,
    visited: HashMap<PathBuf, ModuleId>,
    modules: HashMap<ModuleId, ResolvedModule>,
}

impl ModuleResolver {
    pub fn new(root_path: &Path) -> Self {
        let root_path = root_path
            .canonicalize()
            .unwrap_or_else(|_| root_path.to_path_buf());
        Self {
            root_path,
            next_id: 0,
            visited: HashMap::new(),
            modules: HashMap::new(),
        }
    }

    /// 解析模块路径
    pub fn resolve_path(specifier: &str, parent: &Path) -> Result<PathBuf> {
        // 只支持相对路径
        if !specifier.starts_with('.') {
            bail!(
                "Module specifier '{}' is not supported. Only relative imports (starting with './' or '../') are supported.",
                specifier
            );
        }

        let base = parent.parent().unwrap_or(parent);
        let resolved = base.join(specifier);

        // 尝试添加扩展名
        let candidates = if resolved.extension().is_some() {
            vec![resolved.clone()]
        } else {
            vec![
                resolved.with_extension("js"),
                resolved.with_extension("ts"),
                resolved.clone(),
            ]
        };

        for candidate in &candidates {
            if candidate.exists() {
                return Ok(candidate.canonicalize()?);
            }
        }

        bail!(
            "Cannot find module '{}' from '{}'. Tried: {:?}",
            specifier,
            parent.display(),
            candidates
        );
    }

    /// 解析模块（如果已解析过则返回缓存的 ID）
    pub fn resolve(&mut self, specifier: &str, parent: &Path) -> Result<ModuleId> {
        let path = Self::resolve_path(specifier, parent)?;
        if !path.starts_with(&self.root_path) {
            bail!(
                "Module '{}' resolves outside root '{}': {}",
                specifier,
                self.root_path.display(),
                path.display()
            );
        }

        // 检查缓存
        if let Some(&id) = self.visited.get(&path) {
            return Ok(id);
        }

        // 读取文件
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read module: {}", path.display()))?;

        // 解析 AST
        let ast = wjsm_parser::parse_module(&source)
            .with_context(|| format!("Failed to parse module: {}", path.display()))?;

        // 检测并转换 CommonJS 模块
        let is_cjs = crate::cjs_transform::is_commonjs_module(&ast);
        let ast = if is_cjs {
            let prefix = format!("_{}_", self.next_id);
            crate::cjs_transform::transform_with_prefix(&ast, &prefix)
        } else {
            ast
        };

        // 提取 import/export
        let imports = Self::extract_imports(&ast);
        let exports = Self::extract_exports(&ast);

        // 提取动态 import() specifier（不合并进 imports，保持图语义正确）
        let dynamic_imports = Self::extract_dynamic_imports(&ast)?;

        // 分配 ID
        let id = ModuleId(self.next_id);
        self.next_id += 1;

        // 保存模块
        let module = ResolvedModule {
            id,
            source,
            path: path.clone(),
            ast,
            imports,
            exports,
            dynamic_imports,
            is_cjs,
        };

        self.visited.insert(path, id);
        self.modules.insert(id, module);

        Ok(id)
    }

    /// 获取已解析的模块
    pub fn get_module(&self, id: ModuleId) -> Option<&ResolvedModule> {
        self.modules.get(&id)
    }

    /// 获取所有已解析的模块
    pub fn all_modules(&self) -> impl Iterator<Item = &ResolvedModule> {
        self.modules.values()
    }

    /// 根据路径查找已解析的模块 ID
    pub fn get_id_by_path(&self, path: &Path) -> Option<ModuleId> {
        self.visited.get(path).copied()
    }

    /// 为指定模块添加合成默认导出（如果它没有默认导出但有其他导出）
    pub fn ensure_default_export_for(&mut self, module_id: ModuleId) -> Result<()> {
        let module = self
            .modules
            .get_mut(&module_id)
            .ok_or_else(|| anyhow::anyhow!("invalid ModuleId: {:?}", module_id))?;
        let has_default = module
            .exports
            .iter()
            .any(|e| matches!(e, ExportEntry::Default { .. }));
        if !has_default && !module.exports.is_empty() {
            Self::add_synthetic_default_export(&mut module.ast, &module.exports);
            module.exports = Self::extract_exports(&module.ast);
        }
        Ok(())
    }

    /// 为没有默认导出的模块添加合成默认导出
    fn add_synthetic_default_export(ast: &mut ast::Module, exports: &[ExportEntry]) {
        use swc_core::common::{DUMMY_SP, SyntaxContext};

        let mut export_names: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for entry in exports {
            match entry {
                ExportEntry::Named { exported, .. } => {
                    if exported != "default" && seen.insert(exported.clone()) {
                        export_names.push(exported.clone());
                    }
                }
                ExportEntry::Declaration { name } => {
                    if seen.insert(name.clone()) {
                        export_names.push(name.clone());
                    }
                }
                _ => {}
            }
        }

        if export_names.is_empty() {
            return;
        }

        // 创建合成默认导出表达式：export default { name1, name2, ... }
        let props: Vec<ast::PropOrSpread> = export_names
            .iter()
            .map(|name| {
                ast::PropOrSpread::Prop(Box::new(ast::Prop::Shorthand(ast::Ident::new(
                    name.clone().into(),
                    DUMMY_SP,
                    SyntaxContext::default(),
                ))))
            })
            .collect();

        let default_export = ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDefaultExpr(
            ast::ExportDefaultExpr {
                span: DUMMY_SP,
                expr: Box::new(ast::Expr::Object(ast::ObjectLit {
                    span: DUMMY_SP,
                    props,
                })),
            },
        ));

        ast.body.push(default_export);
    }

    /// 从 AST 中提取 import 声明
    fn extract_imports(module: &ast::Module) -> Vec<ImportEntry> {
        module
            .body
            .iter()
            .filter_map(|item| match item {
                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import_decl)) => {
                    let specifier = import_decl.src.value.to_string_lossy().into_owned();
                    let mut names = Vec::new();

                    for spec in &import_decl.specifiers {
                        match spec {
                            ast::ImportSpecifier::Named(named) => {
                                let local = named.local.sym.to_string();
                                let imported = named
                                    .imported
                                    .as_ref()
                                    .map(|id| match id {
                                        ast::ModuleExportName::Ident(ident) => {
                                            ident.sym.to_string()
                                        }
                                        ast::ModuleExportName::Str(s) => {
                                            s.value.to_string_lossy().into_owned()
                                        }
                                    })
                                    .unwrap_or_else(|| local.clone());
                                names.push((local, imported));
                            }
                            ast::ImportSpecifier::Default(default) => {
                                let local = default.local.sym.to_string();
                                names.push((local, "default".to_string()));
                            }
                            ast::ImportSpecifier::Namespace(ns) => {
                                let local = ns.local.sym.to_string();
                                names.push((local, "*".to_string()));
                            }
                        }
                    }

                    Some(ImportEntry {
                        specifier,
                        names,
                        source_span: import_decl.span,
                    })
                }
                _ => None,
            })
            .collect()
    }
    /// 从 AST 中提取动态 import() 的 specifier
    ///
    /// 遍历 AST 中所有 CallExpr，检测 Callee::Import 的调用，
    /// 提取静态可分析的 specifier 字符串。
    /// - 字符串字面量 → 直接提取
    /// - 无插值模板字符串 → 静态求值
    /// - 有插值模板字符串 → 编译报错
    /// - 其他表达式类型 → 编译报错
    pub fn extract_dynamic_imports(module: &ast::Module) -> Result<Vec<String>> {
        let mut specifiers = Vec::new();
        for item in &module.body {
            Self::extract_dynamic_imports_from_item(item, &mut specifiers)?;
        }
        Ok(specifiers)
    }
 
    fn extract_dynamic_imports_from_item(
        item: &ast::ModuleItem,
        specifiers: &mut Vec<String>,
    ) -> Result<()> {
        match item {
            ast::ModuleItem::ModuleDecl(decl) => {
                Self::extract_dynamic_imports_from_module_decl(decl, specifiers)?;
            }
            ast::ModuleItem::Stmt(stmt) => {
                Self::extract_dynamic_imports_from_stmt(stmt, specifiers)?;
            }
        }
        Ok(())
    }
 
    fn extract_dynamic_imports_from_module_decl(
        decl: &ast::ModuleDecl,
        specifiers: &mut Vec<String>,
    ) -> Result<()> {
        match decl {
            ast::ModuleDecl::ExportDecl(export_decl) => {
                Self::extract_dynamic_imports_from_decl(&export_decl.decl, specifiers)?;
            }
            ast::ModuleDecl::ExportDefaultExpr(default_expr) => {
                Self::extract_dynamic_imports_from_expr(&default_expr.expr, specifiers)?;
            }
            _ => {}
        }
        Ok(())
    }
 
    fn extract_dynamic_imports_from_decl(
        decl: &ast::Decl,
        specifiers: &mut Vec<String>,
    ) -> Result<()> {
        match decl {
            ast::Decl::Fn(fn_decl) => {
                Self::extract_dynamic_imports_from_function(&fn_decl.function, specifiers)?;
            }
            ast::Decl::Class(class_decl) => {
                Self::extract_dynamic_imports_from_class(&class_decl.class, specifiers)?;
            }
            ast::Decl::Var(var_decl) => {
                for declarator in &var_decl.decls {
                    if let Some(init) = &declarator.init {
                        Self::extract_dynamic_imports_from_expr(init, specifiers)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
 
    fn extract_dynamic_imports_from_stmt(
        stmt: &ast::Stmt,
        specifiers: &mut Vec<String>,
    ) -> Result<()> {
        match stmt {
            ast::Stmt::Expr(expr_stmt) => {
                Self::extract_dynamic_imports_from_expr(&expr_stmt.expr, specifiers)?;
            }
            ast::Stmt::Decl(decl) => {
                Self::extract_dynamic_imports_from_decl(decl, specifiers)?;
            }
            ast::Stmt::Block(block) => {
                for s in &block.stmts {
                    Self::extract_dynamic_imports_from_stmt(s, specifiers)?;
                }
            }
            ast::Stmt::If(if_stmt) => {
                Self::extract_dynamic_imports_from_expr(&if_stmt.test, specifiers)?;
                Self::extract_dynamic_imports_from_stmt(&if_stmt.cons, specifiers)?;
                if let Some(alt) = &if_stmt.alt {
                    Self::extract_dynamic_imports_from_stmt(alt, specifiers)?;
                }
            }
            ast::Stmt::For(for_stmt) => {
                if let Some(init) = &for_stmt.init {
                    match init {
                        ast::VarDeclOrExpr::VarDecl(var_decl) => {
                            Self::extract_dynamic_imports_from_decl(
                                &ast::Decl::Var(var_decl.clone()),
                                specifiers,
                            )?;
                        }
                        ast::VarDeclOrExpr::Expr(expr) => {
                            Self::extract_dynamic_imports_from_expr(expr, specifiers)?;
                        }
                    }
                }
                if let Some(test) = &for_stmt.test {
                    Self::extract_dynamic_imports_from_expr(test, specifiers)?;
                }
                if let Some(update) = &for_stmt.update {
                    Self::extract_dynamic_imports_from_expr(update, specifiers)?;
                }
                Self::extract_dynamic_imports_from_stmt(&for_stmt.body, specifiers)?;
            }
            ast::Stmt::ForIn(for_in) => {
                Self::extract_dynamic_imports_from_stmt(&for_in.body, specifiers)?;
            }
            ast::Stmt::ForOf(for_of) => {
                Self::extract_dynamic_imports_from_stmt(&for_of.body, specifiers)?;
            }
            ast::Stmt::While(while_stmt) => {
                Self::extract_dynamic_imports_from_expr(&while_stmt.test, specifiers)?;
                Self::extract_dynamic_imports_from_stmt(&while_stmt.body, specifiers)?;
            }
            ast::Stmt::DoWhile(do_while) => {
                Self::extract_dynamic_imports_from_stmt(&do_while.body, specifiers)?;
                Self::extract_dynamic_imports_from_expr(&do_while.test, specifiers)?;
            }
            ast::Stmt::Switch(switch) => {
                Self::extract_dynamic_imports_from_expr(&switch.discriminant, specifiers)?;
                for case in &switch.cases {
                    for s in &case.cons {
                        Self::extract_dynamic_imports_from_stmt(s, specifiers)?;
                    }
                }
            }
            ast::Stmt::Try(try_stmt) => {
                Self::extract_dynamic_imports_from_stmt(
                    &ast::Stmt::Block(try_stmt.block.clone()),
                    specifiers,
                )?;
                if let Some(handler) = &try_stmt.handler {
                    Self::extract_dynamic_imports_from_stmt(
                        &ast::Stmt::Block(handler.body.clone()),
                        specifiers,
                    )?;
                }
                if let Some(finalizer) = &try_stmt.finalizer {
                    Self::extract_dynamic_imports_from_stmt(
                        &ast::Stmt::Block(finalizer.clone()),
                        specifiers,
                    )?;
                }
            }
            ast::Stmt::Labeled(labeled) => {
                Self::extract_dynamic_imports_from_stmt(&labeled.body, specifiers)?;
            }
            _ => {}
        }
        Ok(())
    }
 
    fn extract_dynamic_imports_from_expr(
        expr: &ast::Expr,
        specifiers: &mut Vec<String>,
    ) -> Result<()> {
        match expr {
            ast::Expr::Call(call) => {
                // 检测 import() 调用
                if matches!(call.callee, ast::Callee::Import(_)) {
                    let specifier = Self::extract_import_call_specifier(call)?;
                    specifiers.push(specifier);
                } else {
                    // 递归进入被调用者和参数
                    if let ast::Callee::Expr(callee_expr) = &call.callee {
                        Self::extract_dynamic_imports_from_expr(callee_expr, specifiers)?;
                    }
                    for arg in &call.args {
                        Self::extract_dynamic_imports_from_expr(&arg.expr, specifiers)?;
                    }
                }
            }
            ast::Expr::Bin(bin) => {
                Self::extract_dynamic_imports_from_expr(&bin.left, specifiers)?;
                Self::extract_dynamic_imports_from_expr(&bin.right, specifiers)?;
            }
            ast::Expr::Unary(unary) => {
                Self::extract_dynamic_imports_from_expr(&unary.arg, specifiers)?;
            }
            ast::Expr::Assign(assign) => {
                Self::extract_dynamic_imports_from_expr(assign.right.as_ref(), specifiers)?;
            }
            ast::Expr::Cond(cond) => {
                Self::extract_dynamic_imports_from_expr(&cond.test, specifiers)?;
                Self::extract_dynamic_imports_from_expr(&cond.cons, specifiers)?;
                Self::extract_dynamic_imports_from_expr(&cond.alt, specifiers)?;
            }
            ast::Expr::Member(member) => {
                Self::extract_dynamic_imports_from_expr(&member.obj, specifiers)?;
                // 计算成员属性：obj[import(...)] 中的表达式也可能包含动态 import
                if let ast::MemberProp::Computed(computed) = &member.prop {
                    Self::extract_dynamic_imports_from_expr(&computed.expr, specifiers)?;
                }
            }
            ast::Expr::Object(obj) => {
                for prop in &obj.props {
                    match prop {
                        ast::PropOrSpread::Prop(p) => match p.as_ref() {
                            ast::Prop::KeyValue(kv) => {
                                Self::extract_dynamic_imports_from_expr(&kv.value, specifiers)?;
                            }
                            ast::Prop::Shorthand(_) => {}
                            ast::Prop::Getter(getter) => {
                                if let Some(body) = &getter.body {
                                    for s in &body.stmts {
                                        Self::extract_dynamic_imports_from_stmt(s, specifiers)?;
                                    }
                                }
                            }
                            ast::Prop::Setter(setter) => {
                                if let Some(body) = &setter.body {
                                    for s in &body.stmts {
                                        Self::extract_dynamic_imports_from_stmt(s, specifiers)?;
                                    }
                                }
                            }
                            ast::Prop::Method(method) => {
                                Self::extract_dynamic_imports_from_function(
                                    &method.function, specifiers,
                                )?;
                            }
                            _ => {}
                        },
                        ast::PropOrSpread::Spread(spread) => {
                            Self::extract_dynamic_imports_from_expr(&spread.expr, specifiers)?;
                        }
                    }
                }
            }
            ast::Expr::Array(arr) => {
                for elem in &arr.elems {
                    if let Some(elem) = elem {
                        Self::extract_dynamic_imports_from_expr(&elem.expr, specifiers)?;
                    }
                }
            }
            ast::Expr::Arrow(arrow) => {
                match &*arrow.body {
                    ast::BlockStmtOrExpr::BlockStmt(block) => {
                        for s in &block.stmts {
                            Self::extract_dynamic_imports_from_stmt(s, specifiers)?;
                        }
                    }
                    ast::BlockStmtOrExpr::Expr(expr) => {
                        Self::extract_dynamic_imports_from_expr(expr, specifiers)?;
                    }
                }
            }
            ast::Expr::Fn(fn_expr) => {
                Self::extract_dynamic_imports_from_function(&fn_expr.function, specifiers)?;
            }
            ast::Expr::Class(class_expr) => {
                Self::extract_dynamic_imports_from_class(&class_expr.class, specifiers)?;
            }
            ast::Expr::Tpl(tpl) => {
                for expr in &tpl.exprs {
                    Self::extract_dynamic_imports_from_expr(expr, specifiers)?;
                }
            }
            ast::Expr::TaggedTpl(tagged) => {
                Self::extract_dynamic_imports_from_expr(&tagged.tag, specifiers)?;
                for expr in &tagged.tpl.exprs {
                    Self::extract_dynamic_imports_from_expr(expr, specifiers)?;
                }
            }
            ast::Expr::Paren(paren) => {
                Self::extract_dynamic_imports_from_expr(&paren.expr, specifiers)?;
            }
            ast::Expr::Seq(seq) => {
                for expr in &seq.exprs {
                    Self::extract_dynamic_imports_from_expr(expr, specifiers)?;
                }
            }
            ast::Expr::New(new) => {
                Self::extract_dynamic_imports_from_expr(&new.callee, specifiers)?;
                if let Some(args) = &new.args {
                    for arg in args {
                        Self::extract_dynamic_imports_from_expr(&arg.expr, specifiers)?;
                    }
                }
            }
            ast::Expr::Await(await_expr) => {
                Self::extract_dynamic_imports_from_expr(&await_expr.arg, specifiers)?;
            }
            ast::Expr::Yield(yield_expr) => {
                if let Some(arg) = &yield_expr.arg {
                    Self::extract_dynamic_imports_from_expr(arg, specifiers)?;
                }
            }
            ast::Expr::MetaProp(_) | ast::Expr::Ident(_) | ast::Expr::Lit(_) => {}
            _ => {}
        }
        Ok(())
    }
 
    fn extract_dynamic_imports_from_function(
        function: &ast::Function,
        specifiers: &mut Vec<String>,
    ) -> Result<()> {
        if let Some(body) = &function.body {
            for s in &body.stmts {
                Self::extract_dynamic_imports_from_stmt(s, specifiers)?;
            }
        }
        Ok(())
    }
 
    fn extract_dynamic_imports_from_class(
        class: &ast::Class,
        specifiers: &mut Vec<String>,
    ) -> Result<()> {
        for member in &class.body {
            match member {
                ast::ClassMember::Method(method) => {
                    Self::extract_dynamic_imports_from_function(&method.function, specifiers)?;
                }
                ast::ClassMember::Constructor(ctor) => {
                    if let Some(body) = &ctor.body {
                        for s in &body.stmts {
                            Self::extract_dynamic_imports_from_stmt(s, specifiers)?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
 
    fn extract_import_call_specifier(call: &ast::CallExpr) -> Result<String> {
        let first_arg = call.args.first().ok_or_else(|| {
            anyhow::anyhow!(
                "import() requires a module specifier; \
                 in AOT compilation mode, only string literal specifiers are supported"
            )
        })?;

        match first_arg.expr.as_ref() {
            ast::Expr::Lit(ast::Lit::Str(s)) => {
                Ok(s.value.to_string_lossy().into_owned())
            }
            ast::Expr::Tpl(tpl) => {
                if tpl.exprs.is_empty() {
                    // 无插值的模板字符串：静态求值
                    let mut result = String::new();
                    for quasi in &tpl.quasis {
                        result.push_str(&quasi.raw);
                    }
                    Ok(result)
                } else {
                    bail!(
                        "import() with template literal containing expressions is not supported; \
                         AOT compilation requires the specifier to be a static string literal"
                    )
                }
            }
            _ => {
                bail!(
                    "import() requires a string literal specifier; \
                     AOT compilation cannot resolve dynamic specifiers at compile time. \
                     Use a string literal like import('./module.js') instead"
                )
            }
        }
    }
    /// 从 AST 中提取 export 声明
    fn extract_exports(module: &ast::Module) -> Vec<ExportEntry> {
        let mut exports = Vec::new();

        for item in &module.body {
            match item {
                ast::ModuleItem::ModuleDecl(decl) => match decl {
                    ast::ModuleDecl::ExportNamed(named_export) => {
                        if let Some(src) = &named_export.src {
                            // export { ... } from './foo' — 命名重导出
                            let source = src.value.to_string_lossy().into_owned();
                            for spec in &named_export.specifiers {
                                match spec {
                                    ast::ExportSpecifier::Named(named) => {
                                        let local = match &named.orig {
                                            ast::ModuleExportName::Ident(ident) => {
                                                ident.sym.to_string()
                                            }
                                            ast::ModuleExportName::Str(s) => {
                                                s.value.to_string_lossy().into_owned()
                                            }
                                        };
                                        let exported = named
                                            .exported
                                            .as_ref()
                                            .map(|id| match id {
                                                ast::ModuleExportName::Ident(ident) => {
                                                    ident.sym.to_string()
                                                }
                                                ast::ModuleExportName::Str(s) => {
                                                    s.value.to_string_lossy().into_owned()
                                                }
                                            })
                                            .unwrap_or_else(|| local.clone());
                                        exports.push(ExportEntry::NamedReExport {
                                            local,
                                            exported,
                                            source: source.clone(),
                                        });
                                    }
                                    ast::ExportSpecifier::Namespace(ns) => {
                                        // export * as ns from './foo'
                                        let name = match &ns.name {
                                            ast::ModuleExportName::Ident(ident) => {
                                                ident.sym.to_string()
                                            }
                                            ast::ModuleExportName::Str(s) => {
                                                s.value.to_string_lossy().into_owned()
                                            }
                                        };
                                        exports.push(ExportEntry::NamedReExport {
                                            local: "*".to_string(),
                                            exported: name,
                                            source: source.clone(),
                                        });
                                    }
                                    ast::ExportSpecifier::Default(default) => {
                                        // export { default } from './foo'
                                        let local = default.exported.sym.to_string();
                                        exports.push(ExportEntry::NamedReExport {
                                            local: local.clone(),
                                            exported: "default".to_string(),
                                            source: source.clone(),
                                        });
                                    }
                                }
                            }
                        } else {
                            // export { x } / export { x as y }
                            for spec in &named_export.specifiers {
                                match spec {
                                    ast::ExportSpecifier::Named(named) => {
                                        let local = match &named.orig {
                                            ast::ModuleExportName::Ident(ident) => {
                                                ident.sym.to_string()
                                            }
                                            ast::ModuleExportName::Str(s) => {
                                                s.value.to_string_lossy().into_owned()
                                            }
                                        };
                                        let exported = named
                                            .exported
                                            .as_ref()
                                            .map(|id| match id {
                                                ast::ModuleExportName::Ident(ident) => {
                                                    ident.sym.to_string()
                                                }
                                                ast::ModuleExportName::Str(s) => {
                                                    s.value.to_string_lossy().into_owned()
                                                }
                                            })
                                            .unwrap_or_else(|| local.clone());
                                        exports.push(ExportEntry::Named { local, exported });
                                    }
                                    ast::ExportSpecifier::Default(default) => {
                                        // export { x as default }
                                        let local = default.exported.sym.to_string();
                                        exports.push(ExportEntry::Named {
                                            local,
                                            exported: "default".to_string(),
                                        });
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    ast::ModuleDecl::ExportDefaultExpr(_default_expr) => {
                        // export default expr
                        // 需要生成一个内部变量名
                        exports.push(ExportEntry::Default {
                            local: "_default_export".to_string(),
                        });
                    }
                    ast::ModuleDecl::ExportDefaultDecl(default_decl) => {
                        // export default function/class
                        let local = match &default_decl.decl {
                            ast::DefaultDecl::Class(class) => class
                                .ident
                                .as_ref()
                                .map(|i| i.sym.to_string())
                                .unwrap_or_else(|| "_default_export".to_string()),
                            ast::DefaultDecl::Fn(func) => func
                                .ident
                                .as_ref()
                                .map(|i| i.sym.to_string())
                                .unwrap_or_else(|| "_default_export".to_string()),
                            ast::DefaultDecl::TsInterfaceDecl(_) => "_default_export".to_string(),
                        };
                        exports.push(ExportEntry::Default { local });
                    }
                    ast::ModuleDecl::ExportAll(all) => {
                        // export * from './foo'
                        exports.push(ExportEntry::All {
                            source: all.src.value.to_string_lossy().into_owned(),
                        });
                    }
                    ast::ModuleDecl::ExportDecl(export_decl) => {
                        // export const/let/var/function/class
                        let name = match &export_decl.decl {
                            ast::Decl::Class(class) => class.ident.sym.to_string(),
                            ast::Decl::Fn(func) => func.ident.sym.to_string(),
                            ast::Decl::Var(var) => {
                                // 变量声明可能有多个
                                for decl in &var.decls {
                                    if let ast::Pat::Ident(ident) = &decl.name {
                                        exports.push(ExportEntry::Declaration {
                                            name: ident.id.sym.to_string(),
                                        });
                                    }
                                }
                                continue;
                            }
                            ast::Decl::TsInterface(_) => continue,
                            ast::Decl::TsTypeAlias(_) => continue,
                            ast::Decl::TsEnum(_) => continue,
                            ast::Decl::TsModule(_) => continue,
                            ast::Decl::Using(_) => continue,
                        };
                        exports.push(ExportEntry::Declaration { name });
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        exports
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn create_temp_project(case: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be monotonic enough for tests")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "wjsm_module_resolver_{case}_{}_{}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&dir).expect("temp project dir should be creatable");
        dir
    }

    fn write_file(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent dir should be created");
        }
        std::fs::write(path, content).expect("fixture file should be writable");
    }

    #[test]
    fn resolve_path_rejects_non_relative_specifier() {
        let root = create_temp_project("non_relative");
        let parent = root.join("main.js");
        let result = ModuleResolver::resolve_path("lodash", &parent);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not supported"));
    }

    #[test]
    fn resolve_path_finds_js_extension() {
        let root = create_temp_project("js_ext");
        write_file(&root, "dep.js", "export const x = 1;\n");
        let parent = root.join("main.js");
        let result = ModuleResolver::resolve_path("./dep", &parent);
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.to_string_lossy().ends_with("dep.js"));
    }

    #[test]
    fn resolve_path_finds_file_with_extension() {
        let root = create_temp_project("with_ext");
        write_file(&root, "dep.js", "export const x = 1;\n");
        let parent = root.join("main.js");
        let result = ModuleResolver::resolve_path("./dep.js", &parent);
        assert!(result.is_ok());
    }

    #[test]
    fn resolve_path_fails_when_module_not_found() {
        let root = create_temp_project("not_found");
        let parent = root.join("main.js");
        let result = ModuleResolver::resolve_path("./nonexistent", &parent);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Cannot find module"));
    }

    #[test]
    fn resolve_path_resolves_parent_directory() {
        let root = create_temp_project("parent_dir");
        write_file(&root, "sibling.js", "export const x = 1;\n");
        let sub_dir = root.join("sub");
        std::fs::create_dir_all(&sub_dir).expect("sub dir should be created");
        let parent = sub_dir.join("main.js");
        let result = ModuleResolver::resolve_path("../sibling", &parent);
        assert!(result.is_ok());
    }

    #[test]
    fn resolve_rejects_path_outside_root() {
        let root = create_temp_project("outside_root");
        let outside_name = format!("{}_sibling", root.file_name().unwrap().to_string_lossy());
        let outside = root.parent().unwrap().join(&outside_name);
        std::fs::create_dir_all(&outside).expect("outside dir should be created");
        write_file(&outside, "dep.js", "export const x = 1;\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);

        let result = resolver.resolve(&format!("../{outside_name}/dep.js"), &parent);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("outside root"));
    }

    #[test]
    fn resolve_returns_cached_id_on_second_call() {
        let root = create_temp_project("cache_test");
        write_file(&root, "dep.js", "export const x = 1;\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id1 = resolver
            .resolve("./dep.js", &parent)
            .expect("first resolve should succeed");
        let id2 = resolver
            .resolve("./dep.js", &parent)
            .expect("second resolve should succeed");
        assert_eq!(id1, id2, "cached resolve should return same ID");
    }

    #[test]
    fn resolve_detects_cjs_module() {
        let root = create_temp_project("cjs_detect");
        write_file(&root, "cjs.js", "module.exports.x = 1;\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./cjs.js", &parent)
            .expect("resolve should succeed");
        let module = resolver.get_module(id).expect("module should exist");
        assert!(module.is_cjs, "module should be detected as CJS");
    }

    #[test]
    fn resolve_parses_esm_module() {
        let root = create_temp_project("esm_detect");
        write_file(&root, "esm.js", "export const x = 1;\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./esm.js", &parent)
            .expect("resolve should succeed");
        let module = resolver.get_module(id).expect("module should exist");
        assert!(!module.is_cjs, "module should not be detected as CJS");
    }

    #[test]
    fn get_module_returns_some_for_existing() {
        let root = create_temp_project("get_mod_some");
        write_file(&root, "dep.js", "export const x = 1;\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./dep.js", &parent)
            .expect("resolve should succeed");
        assert!(resolver.get_module(id).is_some());
    }

    #[test]
    fn get_module_returns_none_for_missing() {
        let root = create_temp_project("get_mod_none");
        let resolver = ModuleResolver::new(&root);
        assert!(resolver.get_module(ModuleId(999)).is_none());
    }

    #[test]
    fn all_modules_iterates_all() {
        let root = create_temp_project("all_mods");
        write_file(&root, "a.js", "export const a = 1;\n");
        write_file(&root, "b.js", "export const b = 2;\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        resolver
            .resolve("./a.js", &parent)
            .expect("resolve a should succeed");
        resolver
            .resolve("./b.js", &parent)
            .expect("resolve b should succeed");
        let count = resolver.all_modules().count();
        assert_eq!(count, 2);
    }

    #[test]
    fn get_id_by_path_returns_some_for_visited() {
        let root = create_temp_project("id_by_path_some");
        write_file(&root, "dep.js", "export const x = 1;\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./dep.js", &parent)
            .expect("resolve should succeed");
        let dep_path = root
            .join("dep.js")
            .canonicalize()
            .expect("canonicalize should work");
        assert_eq!(resolver.get_id_by_path(&dep_path), Some(id));
    }

    #[test]
    fn get_id_by_path_returns_none_for_unknown() {
        let root = create_temp_project("id_by_path_none");
        let resolver = ModuleResolver::new(&root);
        let unknown_path = root.join("nonexistent.js");
        assert!(resolver.get_id_by_path(&unknown_path).is_none());
    }

    #[test]
    fn ensure_default_export_adds_when_no_default() {
        let root = create_temp_project("ensure_default_add");
        write_file(&root, "dep.js", "export const x = 1;\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./dep.js", &parent)
            .expect("resolve should succeed");
        let before_count = resolver.get_module(id).unwrap().exports.len();
        resolver
            .ensure_default_export_for(id)
            .expect("ensure default export should succeed");
        let after_count = resolver.get_module(id).unwrap().exports.len();
        assert!(
            after_count > before_count,
            "should have added a default export"
        );
    }

    #[test]
    fn ensure_default_export_skips_when_has_default() {
        let root = create_temp_project("ensure_default_skip_has");
        write_file(&root, "dep.js", "export default 42;\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./dep.js", &parent)
            .expect("resolve should succeed");
        let before_count = resolver.get_module(id).unwrap().exports.len();
        resolver
            .ensure_default_export_for(id)
            .expect("ensure default export should succeed");
        let after_count = resolver.get_module(id).unwrap().exports.len();
        assert_eq!(
            after_count, before_count,
            "should not add default export when one exists"
        );
    }

    #[test]
    fn ensure_default_export_skips_when_no_exports() {
        let root = create_temp_project("ensure_default_skip_empty");
        write_file(&root, "dep.js", "const x = 1;\nconsole.log(x);\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./dep.js", &parent)
            .expect("resolve should succeed");
        let before_count = resolver.get_module(id).unwrap().exports.len();
        assert_eq!(before_count, 0, "module should have no exports");
        resolver
            .ensure_default_export_for(id)
            .expect("ensure default export should succeed");
        let after_count = resolver.get_module(id).unwrap().exports.len();
        assert_eq!(
            after_count, 0,
            "should not add default export when no exports exist"
        );
    }

    #[test]
    fn extract_imports_handles_named_import() {
        let root = create_temp_project("import_named");
        write_file(&root, "dep.js", "export const x = 1;\n");
        write_file(
            &root,
            "main.js",
            "import { x } from './dep.js';\nconsole.log(x);\n",
        );
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./main.js", &parent)
            .expect("resolve should succeed");
        let module = resolver.get_module(id).expect("module should exist");
        assert_eq!(module.imports.len(), 1);
        assert_eq!(
            module.imports[0].names,
            vec![("x".to_string(), "x".to_string())]
        );
    }

    #[test]
    fn extract_imports_handles_default_import() {
        let root = create_temp_project("import_default");
        write_file(&root, "dep.js", "export default 42;\n");
        write_file(
            &root,
            "main.js",
            "import answer from './dep.js';\nconsole.log(answer);\n",
        );
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./main.js", &parent)
            .expect("resolve should succeed");
        let module = resolver.get_module(id).expect("module should exist");
        assert_eq!(module.imports.len(), 1);
        assert_eq!(
            module.imports[0].names,
            vec![("answer".to_string(), "default".to_string())]
        );
    }

    #[test]
    fn extract_imports_handles_namespace_import() {
        let root = create_temp_project("import_ns");
        write_file(&root, "dep.js", "export const x = 1;\n");
        write_file(
            &root,
            "main.js",
            "import * as ns from './dep.js';\nconsole.log(ns);\n",
        );
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./main.js", &parent)
            .expect("resolve should succeed");
        let module = resolver.get_module(id).expect("module should exist");
        assert_eq!(module.imports.len(), 1);
        assert_eq!(
            module.imports[0].names,
            vec![("ns".to_string(), "*".to_string())]
        );
    }

    #[test]
    fn extract_imports_handles_aliased_named_import() {
        let root = create_temp_project("import_alias");
        write_file(&root, "dep.js", "export const x = 1;\n");
        write_file(
            &root,
            "main.js",
            "import { x as y } from './dep.js';\nconsole.log(y);\n",
        );
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./main.js", &parent)
            .expect("resolve should succeed");
        let module = resolver.get_module(id).expect("module should exist");
        assert_eq!(module.imports.len(), 1);
        assert_eq!(
            module.imports[0].names,
            vec![("y".to_string(), "x".to_string())]
        );
    }

    #[test]
    fn extract_exports_handles_named_export() {
        let root = create_temp_project("export_named");
        write_file(&root, "dep.js", "const x = 1;\nexport { x };\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./dep.js", &parent)
            .expect("resolve should succeed");
        let module = resolver.get_module(id).expect("module should exist");
        assert!(
            module
                .exports
                .iter()
                .any(|e| matches!(e, ExportEntry::Named { exported, .. } if exported == "x"))
        );
    }

    #[test]
    fn extract_exports_handles_default_expr_export() {
        let root = create_temp_project("export_default_expr");
        write_file(&root, "dep.js", "export default 99;\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./dep.js", &parent)
            .expect("resolve should succeed");
        let module = resolver.get_module(id).expect("module should exist");
        assert!(
            module
                .exports
                .iter()
                .any(|e| matches!(e, ExportEntry::Default { .. }))
        );
    }

    #[test]
    fn extract_exports_handles_default_fn_export() {
        let root = create_temp_project("export_default_fn");
        write_file(&root, "dep.js", "export default function hello() {}\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./dep.js", &parent)
            .expect("resolve should succeed");
        let module = resolver.get_module(id).expect("module should exist");
        assert!(
            module
                .exports
                .iter()
                .any(|e| matches!(e, ExportEntry::Default { local, .. } if local == "hello"))
        );
    }

    #[test]
    fn extract_exports_handles_default_class_export() {
        let root = create_temp_project("export_default_class");
        write_file(&root, "dep.js", "export default class MyClass {}\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./dep.js", &parent)
            .expect("resolve should succeed");
        let module = resolver.get_module(id).expect("module should exist");
        assert!(
            module
                .exports
                .iter()
                .any(|e| matches!(e, ExportEntry::Default { local, .. } if local == "MyClass"))
        );
    }

    #[test]
    fn extract_exports_handles_declaration_export() {
        let root = create_temp_project("export_decl");
        write_file(
            &root,
            "dep.js",
            "export const x = 1;\nexport function foo() {}\nexport class Bar {}\n",
        );
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./dep.js", &parent)
            .expect("resolve should succeed");
        let module = resolver.get_module(id).expect("module should exist");
        let names: Vec<&str> = module
            .exports
            .iter()
            .filter_map(|e| {
                if let ExportEntry::Declaration { name } = e {
                    Some(name.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(names.contains(&"x"));
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"Bar"));
    }

    #[test]
    fn extract_exports_handles_export_all() {
        let root = create_temp_project("export_all");
        write_file(&root, "base.js", "export const x = 1;\n");
        write_file(&root, "dep.js", "export * from './base.js';\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./dep.js", &parent)
            .expect("resolve should succeed");
        let module = resolver.get_module(id).expect("module should exist");
        assert!(
            module
                .exports
                .iter()
                .any(|e| matches!(e, ExportEntry::All { source } if source == "./base.js"))
        );
    }

    #[test]
    fn extract_exports_handles_re_export_with_source() {
        let root = create_temp_project("re_export");
        write_file(&root, "base.js", "export const x = 1;\n");
        write_file(&root, "dep.js", "export { x } from './base.js';\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./dep.js", &parent)
            .expect("resolve should succeed");
        let module = resolver.get_module(id).expect("module should exist");
        // export { x } from './base.js' 应该产生 NamedReExport，而不是 All
        assert!(module.exports.iter().any(|e| matches!(
            e,
            ExportEntry::NamedReExport { local, exported, source }
                if local == "x" && exported == "x" && source == "./base.js"
        )));
    }

    #[test]
    fn extract_exports_handles_default_anonymous_fn() {
        let root = create_temp_project("export_default_anon_fn");
        write_file(&root, "dep.js", "export default function() {}\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./dep.js", &parent)
            .expect("resolve should succeed");
        let module = resolver.get_module(id).expect("module should exist");
        assert!(module.exports.iter().any(
            |e| matches!(e, ExportEntry::Default { local, .. } if local == "_default_export")
        ));
    }

    #[test]
    fn extract_exports_handles_default_anonymous_class() {
        let root = create_temp_project("export_default_anon_class");
        write_file(&root, "dep.js", "export default class {}\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./dep.js", &parent)
            .expect("resolve should succeed");
        let module = resolver.get_module(id).expect("module should exist");
        assert!(module.exports.iter().any(
            |e| matches!(e, ExportEntry::Default { local, .. } if local == "_default_export")
        ));
    }

    #[test]
    fn extract_exports_handles_multiple_var_declarations() {
        let root = create_temp_project("export_multi_var");
        write_file(&root, "dep.js", "export const a = 1, b = 2;\n");
        let parent = root.join("main.js");
        let mut resolver = ModuleResolver::new(&root);
        let id = resolver
            .resolve("./dep.js", &parent)
            .expect("resolve should succeed");
        let module = resolver.get_module(id).expect("module should exist");
        let names: Vec<&str> = module
            .exports
            .iter()
            .filter_map(|e| {
                if let ExportEntry::Declaration { name } = e {
                    Some(name.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }
}
