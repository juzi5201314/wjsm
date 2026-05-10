// CJS AST 转换器：将 CommonJS 模块转换为 ESM 风格 AST
//
// 转换规则：
// 1. require('./path') → import __cjs_req_N from './path'; 使用 __cjs_req_N 替换
//    - const x = require('./path') → import x from './path'（直接使用用户变量名）
//    - 其他 require() 调用 → import __cjs_req_N from './path'; 替换为标识符引用
// 2. module.exports.x = value → let {prefix}__cjs_x = value; 合成默认导出 { x: {prefix}__cjs_x }
// 3. exports.x = value → 同上
// 4. module.exports = obj → export default obj
// 5. module.exports.nested.deep = value → 当前不支持深层赋值，静默保留原样
// 6. 对于带有命名导出的 CJS 模块，生成合成默认导出：export default { x: var, y: var, ... }

use std::collections::{BTreeMap, HashSet};
use swc_core::common::{DUMMY_SP, SyntaxContext};
use swc_core::ecma::ast;
use swc_core::ecma::visit::{Visit, VisitWith};

pub fn is_commonjs_module(module: &ast::Module) -> bool {
    let mut detector = CjsDetector { found: false };
    module.visit_with(&mut detector);
    detector.found
}

pub fn transform(module: &ast::Module) -> ast::Module {
    transform_with_prefix(module, "")
}

pub fn transform_with_prefix(module: &ast::Module, export_prefix: &str) -> ast::Module {
    let mut collector = RequireCollector {
        require_map: BTreeMap::new(),
        next_req_id: 0,
        direct_imports: HashSet::new(),
    };
    module.visit_with(&mut collector);

    let mut transformer = CjsTransformer {
        require_map: collector.require_map,
        direct_imports: collector.direct_imports,
        export_names: Vec::new(),
        has_default_export: false,
        export_prefix: export_prefix.to_string(),
    };
    let mut new_module = transformer.transform_module(module);

    // 处理命名导出
    if !transformer.export_names.is_empty() {
        if transformer.has_default_export {
            // 两者并存：除了已有的 export default，也为属性生成命名导出
            // 这样 `import { VERSION } from './mod'` 也能工作
            for (prop_name, var_name) in &transformer.export_names {
                let export_spec = ast::ExportNamedSpecifier {
                    span: DUMMY_SP,
                    orig: ast::ModuleExportName::Ident(ast::Ident::new(
                        var_name.clone().into(),
                        DUMMY_SP,
                        SyntaxContext::default(),
                    )),
                    exported: Some(ast::ModuleExportName::Ident(ast::Ident::new(
                        prop_name.clone().into(),
                        DUMMY_SP,
                        SyntaxContext::default(),
                    ))),
                    is_type_only: false,
                };
                new_module
                    .body
                    .push(ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportNamed(
                        ast::NamedExport {
                            span: DUMMY_SP,
                            specifiers: vec![ast::ExportSpecifier::Named(export_spec)],
                            src: None,
                            type_only: false,
                            with: None,
                        },
                    )));
            }
        } else {
            // 只有命名导出：生成合成默认导出
            let default_export_expr = create_synthetic_default_export(&transformer.export_names);
            new_module.body.push(ast::ModuleItem::ModuleDecl(
                ast::ModuleDecl::ExportDefaultExpr(ast::ExportDefaultExpr {
                    span: DUMMY_SP,
                    expr: Box::new(default_export_expr),
                }),
            ));
        }
    }

    new_module
}

// ── CJS 检测器（使用 Visit trait）────────────────────────────────

struct CjsDetector {
    found: bool,
}

impl Visit for CjsDetector {
    fn visit_call_expr(&mut self, n: &ast::CallExpr) {
        if self.found {
            return;
        }
        if extract_require_specifier(n).is_some() {
            self.found = true;
            return;
        }
        n.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, n: &ast::MemberExpr) {
        if self.found {
            return;
        }
        if is_module_exports_member(&ast::Expr::Member(n.clone()))
            || is_exports_member(&ast::Expr::Member(n.clone()))
        {
            self.found = true;
            return;
        }
        n.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, n: &ast::AssignExpr) {
        if self.found {
            return;
        }
        if let ast::AssignTarget::Simple(simple) = &n.left {
            if let ast::SimpleAssignTarget::Member(member) = simple {
                if is_module_exports_member(&member.obj) || is_exports_ident(&member.obj) {
                    self.found = true;
                    return;
                }
                if is_module_exports_member_no_prop(&member.obj, &member.prop) {
                    self.found = true;
                    return;
                }
            }
        }
        n.visit_children_with(self);
    }
}

// ── Require 收集器（使用 Visit trait）─────────────────────────────

struct RequireCollector {
    require_map: BTreeMap<String, String>,
    next_req_id: u32,
    direct_imports: HashSet<String>,
}

impl Visit for RequireCollector {
    fn visit_var_decl(&mut self, n: &ast::VarDecl) {
        for decl in &n.decls {
            if let Some(init) = &decl.init {
                if let ast::Expr::Call(call) = init.as_ref() {
                    if let Some(specifier) = extract_require_specifier(call) {
                        if let ast::Pat::Ident(binding) = &decl.name {
                            let local_name = binding.id.sym.to_string();
                            if !local_name.starts_with("__cjs_req_") {
                                if !self.require_map.contains_key(&specifier) {
                                    self.require_map.insert(specifier, local_name.clone());
                                    self.direct_imports.insert(local_name);
                                }
                                continue;
                            }
                        }
                    }
                }
            }
        }
        n.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, n: &ast::CallExpr) {
        if let Some(specifier) = extract_require_specifier(n) {
            if !self.require_map.contains_key(&specifier) {
                let local_name = format!("__cjs_req_{}", self.next_req_id);
                self.next_req_id += 1;
                self.require_map.insert(specifier, local_name);
            }
            return;
        }
        n.visit_children_with(self);
    }
}

// ── CJS 转换器（手动遍历，处理语义转换）───────────────────────────

struct CjsTransformer {
    require_map: BTreeMap<String, String>,
    direct_imports: HashSet<String>,
    export_names: Vec<(String, String)>,
    has_default_export: bool,
    export_prefix: String,
}

impl CjsTransformer {
    fn transform_module(&mut self, module: &ast::Module) -> ast::Module {
        let mut new_body: Vec<ast::ModuleItem> = Vec::new();

        for (specifier, local_name) in &self.require_map {
            let import_decl = create_import_default_decl(specifier, local_name);
            new_body.push(ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(
                import_decl,
            )));
        }

        for item in &module.body {
            match item {
                ast::ModuleItem::Stmt(stmt) => {
                    self.transform_stmt_into(stmt, &mut new_body);
                }
                ast::ModuleItem::ModuleDecl(_) => {
                    new_body.push(item.clone());
                }
            }
        }

        ast::Module {
            span: module.span,
            body: new_body,
            shebang: module.shebang.clone(),
        }
    }

    fn transform_stmt_into(&mut self, stmt: &ast::Stmt, out: &mut Vec<ast::ModuleItem>) {
        match stmt {
            ast::Stmt::Expr(expr_stmt) => {
                if let Some(items) = self.try_transform_expr_stmt(&expr_stmt.expr) {
                    out.extend(items);
                } else {
                    out.push(ast::ModuleItem::Stmt(ast::Stmt::Expr(ast::ExprStmt {
                        span: expr_stmt.span,
                        expr: Box::new(self.transform_expr(&expr_stmt.expr)),
                    })));
                }
            }
            ast::Stmt::Decl(decl) => {
                out.push(ast::ModuleItem::Stmt(self.transform_decl(decl)));
            }
            other => out.push(ast::ModuleItem::Stmt(self.transform_stmt(other))),
        }
    }

    fn transform_stmt(&mut self, stmt: &ast::Stmt) -> ast::Stmt {
        match stmt {
            ast::Stmt::Expr(expr_stmt) => ast::Stmt::Expr(ast::ExprStmt {
                span: expr_stmt.span,
                expr: Box::new(self.transform_expr(&expr_stmt.expr)),
            }),
            ast::Stmt::Decl(decl) => self.transform_decl(decl),
            ast::Stmt::Block(block) => {
                let mut items = Vec::new();
                for s in &block.stmts {
                    self.transform_stmt_into(s, &mut items);
                }
                let stmts = items
                    .into_iter()
                    .map(|item| match item {
                        ast::ModuleItem::Stmt(s) => s,
                        ast::ModuleItem::ModuleDecl(decl) => match decl {
                            ast::ModuleDecl::ExportDecl(e) => ast::Stmt::Decl(e.decl),
                            ast::ModuleDecl::ExportDefaultExpr(e) => {
                                ast::Stmt::Expr(ast::ExprStmt {
                                    span: DUMMY_SP,
                                    expr: e.expr,
                                })
                            }
                            _other => ast::Stmt::Empty(ast::EmptyStmt { span: DUMMY_SP }),
                        },
                    })
                    .collect();
                ast::Stmt::Block(ast::BlockStmt {
                    span: block.span,
                    ctxt: SyntaxContext::default(),
                    stmts,
                })
            }
            ast::Stmt::If(if_stmt) => ast::Stmt::If(ast::IfStmt {
                span: if_stmt.span,
                test: Box::new(self.transform_expr(&if_stmt.test)),
                cons: Box::new(self.transform_stmt(&if_stmt.cons)),
                alt: if_stmt
                    .alt
                    .as_ref()
                    .map(|a| Box::new(self.transform_stmt(a))),
            }),
            ast::Stmt::While(w) => ast::Stmt::While(ast::WhileStmt {
                span: w.span,
                test: Box::new(self.transform_expr(&w.test)),
                body: Box::new(self.transform_stmt(&w.body)),
            }),
            ast::Stmt::DoWhile(dw) => ast::Stmt::DoWhile(ast::DoWhileStmt {
                span: dw.span,
                test: Box::new(self.transform_expr(&dw.test)),
                body: Box::new(self.transform_stmt(&dw.body)),
            }),
            ast::Stmt::For(f) => ast::Stmt::For(ast::ForStmt {
                span: f.span,
                init: f.init.as_ref().map(|i| match i {
                    ast::VarDeclOrExpr::VarDecl(v) => {
                        ast::VarDeclOrExpr::VarDecl(Box::new(self.transform_var_decl(v)))
                    }
                    ast::VarDeclOrExpr::Expr(e) => {
                        ast::VarDeclOrExpr::Expr(Box::new(self.transform_expr(e)))
                    }
                }),
                test: f.test.as_ref().map(|e| Box::new(self.transform_expr(e))),
                update: f.update.as_ref().map(|e| Box::new(self.transform_expr(e))),
                body: Box::new(self.transform_stmt(&f.body)),
            }),
            ast::Stmt::ForIn(fi) => ast::Stmt::ForIn(ast::ForInStmt {
                span: fi.span,
                left: fi.left.clone(),
                right: Box::new(self.transform_expr(&fi.right)),
                body: Box::new(self.transform_stmt(&fi.body)),
            }),
            ast::Stmt::ForOf(fo) => ast::Stmt::ForOf(ast::ForOfStmt {
                span: fo.span,
                is_await: fo.is_await,
                left: fo.left.clone(),
                right: Box::new(self.transform_expr(&fo.right)),
                body: Box::new(self.transform_stmt(&fo.body)),
            }),
            ast::Stmt::Switch(sw) => ast::Stmt::Switch(ast::SwitchStmt {
                span: sw.span,
                discriminant: Box::new(self.transform_expr(&sw.discriminant)),
                cases: sw
                    .cases
                    .iter()
                    .map(|c| ast::SwitchCase {
                        span: c.span,
                        test: c.test.as_ref().map(|e| Box::new(self.transform_expr(e))),
                        cons: c.cons.iter().map(|s| self.transform_stmt(s)).collect(),
                    })
                    .collect(),
            }),
            ast::Stmt::Try(tr) => ast::Stmt::Try(Box::new(ast::TryStmt {
                span: tr.span,
                block: self.transform_block(&tr.block),
                handler: tr.handler.as_ref().map(|h| ast::CatchClause {
                    span: h.span,
                    param: h.param.clone(),
                    body: self.transform_block(&h.body),
                }),
                finalizer: tr.finalizer.as_ref().map(|f| self.transform_block(f)),
            })),
            ast::Stmt::Labeled(l) => ast::Stmt::Labeled(ast::LabeledStmt {
                span: l.span,
                label: l.label.clone(),
                body: Box::new(self.transform_stmt(&l.body)),
            }),
            ast::Stmt::Return(r) => ast::Stmt::Return(ast::ReturnStmt {
                span: r.span,
                arg: r.arg.as_ref().map(|e| Box::new(self.transform_expr(e))),
            }),
            ast::Stmt::Throw(t) => ast::Stmt::Throw(ast::ThrowStmt {
                span: t.span,
                arg: Box::new(self.transform_expr(&t.arg)),
            }),
            ast::Stmt::With(w) => ast::Stmt::With(ast::WithStmt {
                span: w.span,
                obj: Box::new(self.transform_expr(&w.obj)),
                body: Box::new(self.transform_stmt(&w.body)),
            }),
            other => other.clone(),
        }
    }

    fn transform_block(&mut self, block: &ast::BlockStmt) -> ast::BlockStmt {
        let mut items = Vec::new();
        for s in &block.stmts {
            self.transform_stmt_into(s, &mut items);
        }
        let stmts = items
            .into_iter()
            .map(|item| match item {
                ast::ModuleItem::Stmt(s) => s,
                ast::ModuleItem::ModuleDecl(decl) => match decl {
                    ast::ModuleDecl::ExportDecl(e) => ast::Stmt::Decl(e.decl),
                    ast::ModuleDecl::ExportDefaultExpr(e) => ast::Stmt::Expr(ast::ExprStmt {
                        span: DUMMY_SP,
                        expr: e.expr,
                    }),
                    _ => ast::Stmt::Empty(ast::EmptyStmt { span: DUMMY_SP }),
                },
            })
            .collect();
        ast::BlockStmt {
            span: block.span,
            ctxt: SyntaxContext::default(),
            stmts,
        }
    }

    /// 转换函数体：递归处理函数体中的 require() 调用
    fn transform_function(&mut self, func: &ast::Function) -> ast::Function {
        let mut new_func = func.clone();
        if let Some(body) = new_func.body.take() {
            new_func.body = Some(self.transform_block(&body));
        }
        new_func
    }

    /// 转换类体：递归处理类方法体中的 require() 调用
    fn transform_class_body(&mut self, body: &mut Vec<ast::ClassMember>) {
        for member in body {
            match member {
                ast::ClassMember::Method(method) => {
                    if let Some(block) = method.function.body.take() {
                        method.function.body = Some(self.transform_block(&block));
                    }
                }
                ast::ClassMember::PrivateMethod(method) => {
                    if let Some(block) = method.function.body.take() {
                        method.function.body = Some(self.transform_block(&block));
                    }
                }
                ast::ClassMember::Constructor(ctor) => {
                    if let Some(block) = ctor.body.take() {
                        ctor.body = Some(self.transform_block(&block));
                    }
                }
                ast::ClassMember::ClassProp(prop) => {
                    if let Some(value) = prop.value.take() {
                        prop.value = Some(Box::new(self.transform_expr(&value)));
                    }
                }
                ast::ClassMember::PrivateProp(prop) => {
                    if let Some(value) = prop.value.take() {
                        prop.value = Some(Box::new(self.transform_expr(&value)));
                    }
                }
                _ => {}
            }
        }
    }

    fn transform_decl(&mut self, decl: &ast::Decl) -> ast::Stmt {
        match decl {
            ast::Decl::Var(var_decl) => {
                let transformed = self.transform_var_decl(var_decl);
                if transformed.decls.is_empty() {
                    ast::Stmt::Empty(ast::EmptyStmt { span: DUMMY_SP })
                } else {
                    ast::Stmt::Decl(ast::Decl::Var(Box::new(transformed)))
                }
            }
            ast::Decl::Fn(fn_decl) => {
                // 递归转换函数体中的 require() 调用
                let mut new_fn_decl = fn_decl.clone();
                new_fn_decl.function = Box::new(self.transform_function(&fn_decl.function));
                ast::Stmt::Decl(ast::Decl::Fn(new_fn_decl))
            }
            ast::Decl::Class(class_decl) => {
                // 递归转换类方法体中的 require() 调用
                let mut class = (*class_decl.class).clone();
                self.transform_class_body(&mut class.body);
                ast::Stmt::Decl(ast::Decl::Class(ast::ClassDecl {
                    ident: class_decl.ident.clone(),
                    declare: class_decl.declare,
                    class: Box::new(class),
                }))
            }
            other => ast::Stmt::Decl(other.clone()),
        }
    }

    fn try_transform_expr_stmt(&mut self, expr: &ast::Expr) -> Option<Vec<ast::ModuleItem>> {
        let ast::Expr::Assign(assign) = expr else {
            return None;
        };
        if assign.op != ast::AssignOp::Assign {
            return None;
        }
        let ast::AssignTarget::Simple(simple) = &assign.left else {
            return None;
        };
        let ast::SimpleAssignTarget::Member(member) = simple else {
            return None;
        };

        // module.exports = value → export default value
        if is_module_exports_member_no_prop(&member.obj, &member.prop) {
            let value = self.transform_expr(&assign.right);
            self.has_default_export = true;
            return Some(vec![ast::ModuleItem::ModuleDecl(
                ast::ModuleDecl::ExportDefaultExpr(ast::ExportDefaultExpr {
                    span: DUMMY_SP,
                    expr: Box::new(value),
                }),
            )]);
        }

        // module.exports.x = value → let {prefix}__cjs_x = value
        // exports.x = value → let {prefix}__cjs_x = value
        let is_mod_exp = is_module_exports_member(&member.obj);
        let is_exp = is_exports_ident(&member.obj);
        if !is_mod_exp && !is_exp {
            return None;
        }

        let prop_name = match &member.prop {
            ast::MemberProp::Ident(ident) => ident.sym.to_string(),
            ast::MemberProp::Computed(computed) => {
                if let ast::Expr::Lit(ast::Lit::Str(s)) = computed.expr.as_ref() {
                    s.value.to_string_lossy().into_owned()
                } else {
                    return None;
                }
            }
            _ => return None,
        };

        let value = self.transform_expr(&assign.right);
        let var_name = format!("{}__cjs_{}", self.export_prefix, prop_name);
        self.export_names.push((prop_name, var_name.clone()));
        let decl = create_let_decl(&var_name, value);
        Some(vec![ast::ModuleItem::Stmt(ast::Stmt::Decl(decl))])
    }

    fn transform_var_decl(&mut self, var_decl: &ast::VarDecl) -> ast::VarDecl {
        let mut new_decls = Vec::new();
        for decl in &var_decl.decls {
            if let Some(init) = &decl.init {
                if let ast::Expr::Call(call) = init.as_ref() {
                    if let Some(specifier) = extract_require_specifier(call) {
                        if let ast::Pat::Ident(binding) = &decl.name {
                            let local_name = binding.id.sym.to_string();
                            if self.require_map.get(&specifier) == Some(&local_name)
                                && self.direct_imports.contains(&local_name)
                            {
                                continue;
                            }
                        }
                    }
                }
            }
            let init = decl.init.as_ref().map(|e| Box::new(self.transform_expr(e)));
            new_decls.push(ast::VarDeclarator {
                span: decl.span,
                name: decl.name.clone(),
                init,
                definite: decl.definite,
            });
        }
        ast::VarDecl {
            span: var_decl.span,
            ctxt: SyntaxContext::default(),
            kind: var_decl.kind,
            declare: var_decl.declare,
            decls: new_decls,
        }
    }

    fn transform_expr(&mut self, expr: &ast::Expr) -> ast::Expr {
        match expr {
            ast::Expr::Call(call) => {
                if let Some(specifier) = extract_require_specifier(call) {
                    if let Some(local_name) = self.require_map.get(&specifier) {
                        return ast::Expr::Ident(ast::Ident::new(
                            local_name.clone().into(),
                            DUMMY_SP,
                            SyntaxContext::default(),
                        ));
                    }
                }
                let new_callee = match &call.callee {
                    ast::Callee::Expr(callee) => {
                        ast::Callee::Expr(Box::new(self.transform_expr(callee)))
                    }
                    other => other.clone(),
                };
                let new_args = call
                    .args
                    .iter()
                    .map(|arg| ast::ExprOrSpread {
                        spread: arg.spread,
                        expr: Box::new(self.transform_expr(&arg.expr)),
                    })
                    .collect();
                ast::Expr::Call(ast::CallExpr {
                    span: call.span,
                    ctxt: SyntaxContext::default(),
                    callee: new_callee,
                    args: new_args,
                    type_args: call.type_args.clone(),
                })
            }
            ast::Expr::Member(member) => ast::Expr::Member(ast::MemberExpr {
                span: member.span,
                obj: Box::new(self.transform_expr(&member.obj)),
                prop: member.prop.clone(),
            }),
            ast::Expr::Bin(bin) => ast::Expr::Bin(ast::BinExpr {
                span: bin.span,
                op: bin.op,
                left: Box::new(self.transform_expr(&bin.left)),
                right: Box::new(self.transform_expr(&bin.right)),
            }),
            ast::Expr::Unary(unary) => ast::Expr::Unary(ast::UnaryExpr {
                span: unary.span,
                op: unary.op,
                arg: Box::new(self.transform_expr(&unary.arg)),
            }),
            ast::Expr::Update(update) => ast::Expr::Update(ast::UpdateExpr {
                span: update.span,
                op: update.op,
                prefix: update.prefix,
                arg: Box::new(self.transform_expr(&update.arg)),
            }),
            ast::Expr::Assign(assign) => {
                let new_left = match &assign.left {
                    ast::AssignTarget::Simple(simple) => match simple {
                        ast::SimpleAssignTarget::Member(m) => ast::AssignTarget::Simple(
                            ast::SimpleAssignTarget::Member(ast::MemberExpr {
                                span: m.span,
                                obj: Box::new(self.transform_expr(&m.obj)),
                                prop: m.prop.clone(),
                            }),
                        ),
                        other => ast::AssignTarget::Simple(other.clone()),
                    },
                    ast::AssignTarget::Pat(pat) => ast::AssignTarget::Pat(pat.clone()),
                };
                ast::Expr::Assign(ast::AssignExpr {
                    span: assign.span,
                    op: assign.op,
                    left: new_left,
                    right: Box::new(self.transform_expr(&assign.right)),
                })
            }
            ast::Expr::Cond(cond) => ast::Expr::Cond(ast::CondExpr {
                span: cond.span,
                test: Box::new(self.transform_expr(&cond.test)),
                cons: Box::new(self.transform_expr(&cond.cons)),
                alt: Box::new(self.transform_expr(&cond.alt)),
            }),
            ast::Expr::Seq(seq) => ast::Expr::Seq(ast::SeqExpr {
                span: seq.span,
                exprs: seq
                    .exprs
                    .iter()
                    .map(|e| Box::new(self.transform_expr(e)))
                    .collect(),
            }),
            ast::Expr::Array(arr) => ast::Expr::Array(ast::ArrayLit {
                span: arr.span,
                elems: arr
                    .elems
                    .iter()
                    .map(|elem| {
                        elem.as_ref().map(|e| ast::ExprOrSpread {
                            spread: e.spread,
                            expr: Box::new(self.transform_expr(&e.expr)),
                        })
                    })
                    .collect(),
            }),
            ast::Expr::Object(obj) => ast::Expr::Object(ast::ObjectLit {
                span: obj.span,
                props: obj
                    .props
                    .iter()
                    .map(|prop| match prop {
                        ast::PropOrSpread::Prop(prop) => {
                            ast::PropOrSpread::Prop(Box::new(match prop.as_ref() {
                                ast::Prop::KeyValue(kv) => ast::Prop::KeyValue(ast::KeyValueProp {
                                    key: kv.key.clone(),
                                    value: Box::new(self.transform_expr(&kv.value)),
                                }),
                                ast::Prop::Assign(a) => ast::Prop::Assign(ast::AssignProp {
                                    span: a.span,
                                    key: a.key.clone(),
                                    value: Box::new(self.transform_expr(&a.value)),
                                }),
                                other => other.clone(),
                            }))
                        }
                        ast::PropOrSpread::Spread(s) => {
                            ast::PropOrSpread::Spread(ast::SpreadElement {
                                dot3_token: s.dot3_token,
                                expr: Box::new(self.transform_expr(&s.expr)),
                            })
                        }
                    })
                    .collect(),
            }),
            ast::Expr::Arrow(arrow) => ast::Expr::Arrow(ast::ArrowExpr {
                span: arrow.span,
                ctxt: SyntaxContext::default(),
                params: arrow.params.clone(),
                body: match arrow.body.as_ref() {
                    ast::BlockStmtOrExpr::BlockStmt(block) => {
                        Box::new(ast::BlockStmtOrExpr::BlockStmt(self.transform_block(block)))
                    }
                    ast::BlockStmtOrExpr::Expr(expr) => Box::new(ast::BlockStmtOrExpr::Expr(
                        Box::new(self.transform_expr(expr)),
                    )),
                },
                is_async: arrow.is_async,
                is_generator: arrow.is_generator,
                type_params: arrow.type_params.clone(),
                return_type: arrow.return_type.clone(),
            }),
            ast::Expr::Paren(paren) => ast::Expr::Paren(ast::ParenExpr {
                span: paren.span,
                expr: Box::new(self.transform_expr(&paren.expr)),
            }),
            ast::Expr::Tpl(tpl) => ast::Expr::Tpl(ast::Tpl {
                span: tpl.span,
                exprs: tpl
                    .exprs
                    .iter()
                    .map(|e| Box::new(self.transform_expr(e)))
                    .collect(),
                quasis: tpl.quasis.clone(),
            }),
            ast::Expr::OptChain(oc) => ast::Expr::OptChain(ast::OptChainExpr {
                span: oc.span,
                optional: oc.optional,
                base: Box::new(match oc.base.as_ref() {
                    ast::OptChainBase::Member(m) => ast::OptChainBase::Member(ast::MemberExpr {
                        span: m.span,
                        obj: Box::new(self.transform_expr(&m.obj)),
                        prop: m.prop.clone(),
                    }),
                    ast::OptChainBase::Call(c) => ast::OptChainBase::Call(ast::OptCall {
                        span: c.span,
                        ctxt: SyntaxContext::default(),
                        callee: Box::new(self.transform_expr(&c.callee)),
                        args: c
                            .args
                            .iter()
                            .map(|a| ast::ExprOrSpread {
                                spread: a.spread,
                                expr: Box::new(self.transform_expr(&a.expr)),
                            })
                            .collect(),
                        type_args: c.type_args.clone(),
                    }),
                }),
            }),
            ast::Expr::New(ne) => ast::Expr::New(ast::NewExpr {
                span: ne.span,
                ctxt: SyntaxContext::default(),
                callee: Box::new(self.transform_expr(&ne.callee)),
                args: ne.args.as_ref().map(|args| {
                    args.iter()
                        .map(|a| ast::ExprOrSpread {
                            spread: a.spread,
                            expr: Box::new(self.transform_expr(&a.expr)),
                        })
                        .collect()
                }),
                type_args: ne.type_args.clone(),
            }),
            ast::Expr::Await(a) => ast::Expr::Await(ast::AwaitExpr {
                span: a.span,
                arg: Box::new(self.transform_expr(&a.arg)),
            }),
            ast::Expr::Yield(y) => ast::Expr::Yield(ast::YieldExpr {
                span: y.span,
                arg: y.arg.as_ref().map(|e| Box::new(self.transform_expr(e))),
                delegate: y.delegate,
            }),
            ast::Expr::Fn(f) => ast::Expr::Fn(ast::FnExpr {
                ident: f.ident.clone(),
                function: Box::new(self.transform_function(&f.function)),
            }),
            ast::Expr::Class(c) => {
                // 递归转换类方法体中的 require() 调用
                let mut class = (*c.class).clone();
                self.transform_class_body(&mut class.body);
                ast::Expr::Class(ast::ClassExpr {
                    ident: c.ident.clone(),
                    class: Box::new(class),
                })
            }
            ast::Expr::TaggedTpl(t) => ast::Expr::TaggedTpl(ast::TaggedTpl {
                span: t.span,
                ctxt: SyntaxContext::default(),
                tag: Box::new(self.transform_expr(&t.tag)),
                tpl: Box::new(ast::Tpl {
                    span: t.tpl.span,
                    exprs: t
                        .tpl
                        .exprs
                        .iter()
                        .map(|e| Box::new(self.transform_expr(e)))
                        .collect(),
                    quasis: t.tpl.quasis.clone(),
                }),
                type_params: t.type_params.clone(),
            }),
            other => other.clone(),
        }
    }
}

// ── 辅助函数 ──────────────────────────────────────────────────────

fn extract_require_specifier(call: &ast::CallExpr) -> Option<String> {
    if let ast::Callee::Expr(expr) = &call.callee {
        if let ast::Expr::Ident(ident) = expr.as_ref() {
            if ident.sym.as_ref() == "require" && call.args.len() == 1 {
                if let ast::Expr::Lit(ast::Lit::Str(s)) = call.args[0].expr.as_ref() {
                    return Some(s.value.to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

fn is_module_exports_member(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Member(member) => {
            if let ast::Expr::Ident(module_ident) = member.obj.as_ref() {
                if module_ident.sym.as_ref() == "module" {
                    if let ast::MemberProp::Ident(exports_ident) = &member.prop {
                        if exports_ident.sym.as_ref() == "exports" {
                            return true;
                        }
                    }
                }
            }
            false
        }
        _ => false,
    }
}

fn is_module_exports_member_no_prop(obj: &ast::Expr, prop: &ast::MemberProp) -> bool {
    if let ast::Expr::Ident(module_ident) = obj {
        if module_ident.sym.as_ref() == "module" {
            if let ast::MemberProp::Ident(exports_ident) = prop {
                if exports_ident.sym.as_ref() == "exports" {
                    return true;
                }
            }
        }
    }
    false
}

fn is_exports_member(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Member(member) => {
            if let ast::Expr::Ident(exports_ident) = member.obj.as_ref() {
                if exports_ident.sym.as_ref() == "exports" {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

fn is_exports_ident(expr: &ast::Expr) -> bool {
    matches!(expr, ast::Expr::Ident(ident) if ident.sym.as_ref() == "exports")
}

fn create_import_default_decl(specifier: &str, local_name: &str) -> ast::ImportDecl {
    ast::ImportDecl {
        span: DUMMY_SP,
        phase: ast::ImportPhase::Evaluation,
        specifiers: vec![ast::ImportSpecifier::Default(ast::ImportDefaultSpecifier {
            span: DUMMY_SP,
            local: ast::Ident::new(local_name.into(), DUMMY_SP, SyntaxContext::default()),
        })],
        src: Box::new(ast::Str {
            span: DUMMY_SP,
            value: specifier.into(),
            raw: None,
        }),
        type_only: false,
        with: None,
    }
}

fn create_synthetic_default_export(export_names: &[(String, String)]) -> ast::Expr {
    let props: Vec<ast::PropOrSpread> = export_names
        .iter()
        .map(|(prop_name, var_name)| {
            ast::PropOrSpread::Prop(Box::new(ast::Prop::KeyValue(ast::KeyValueProp {
                key: ast::PropName::Ident(ast::IdentName::new(prop_name.clone().into(), DUMMY_SP)),
                value: Box::new(ast::Expr::Ident(ast::Ident::new(
                    var_name.clone().into(),
                    DUMMY_SP,
                    SyntaxContext::default(),
                ))),
            })))
        })
        .collect();
    ast::Expr::Object(ast::ObjectLit {
        span: DUMMY_SP,
        props,
    })
}

fn create_let_decl(name: &str, value: ast::Expr) -> ast::Decl {
    ast::Decl::Var(Box::new(ast::VarDecl {
        span: DUMMY_SP,
        ctxt: SyntaxContext::default(),
        kind: ast::VarDeclKind::Let,
        declare: false,
        decls: vec![ast::VarDeclarator {
            span: DUMMY_SP,
            name: ast::Pat::Ident(ast::BindingIdent {
                id: ast::Ident::new(name.into(), DUMMY_SP, SyntaxContext::default()),
                type_ann: None,
            }),
            init: Some(Box::new(value)),
            definite: false,
        }],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_parser;

    fn parse(source: &str) -> ast::Module {
        wjsm_parser::parse_module(source).expect("parse should succeed")
    }

    fn has_import_with_local(transformed: &ast::Module, local: &str) -> bool {
        transformed.body.iter().any(|item| {
            if let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import)) = item {
                import.specifiers.iter().any(|s| {
                    if let ast::ImportSpecifier::Default(d) = s {
                        d.local.sym.as_ref() == local
                    } else {
                        false
                    }
                })
            } else {
                false
            }
        })
    }

    fn has_let_decl(transformed: &ast::Module) -> bool {
        transformed.body.iter().any(|item| {
            if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) = item {
                var.kind == ast::VarDeclKind::Let
            } else {
                false
            }
        })
    }

    fn has_default_export(transformed: &ast::Module) -> bool {
        transformed.body.iter().any(|item| {
            matches!(
                item,
                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDefaultExpr(_))
            )
        })
    }

    fn has_import_decl(transformed: &ast::Module) -> bool {
        transformed.body.iter().any(|item| {
            matches!(
                item,
                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(_))
            )
        })
    }

    #[test]
    fn detects_commonjs_require() {
        let module = parse(r#"const foo = require('./foo');"#);
        assert!(is_commonjs_module(&module));
    }

    #[test]
    fn detects_commonjs_module_exports() {
        let module = parse(r#"module.exports.foo = 1;"#);
        assert!(is_commonjs_module(&module));
    }

    #[test]
    fn detects_commonjs_exports() {
        let module = parse(r#"exports.bar = 2;"#);
        assert!(is_commonjs_module(&module));
    }

    #[test]
    fn does_not_detect_plain_module() {
        let module = parse(r#"const x = 1; console.log(x);"#);
        assert!(!is_commonjs_module(&module));
    }

    #[test]
    fn detects_cjs_via_assign_to_exports_ident() {
        let module = parse(r#"exports.foo = 1;"#);
        assert!(is_commonjs_module(&module));
    }

    #[test]
    fn does_not_detect_cjs_for_member_access_only() {
        let module = parse(r#"const x = 1; console.log(x);"#);
        assert!(!is_commonjs_module(&module));
    }

    #[test]
    fn transforms_require() {
        let module = parse(r#"const foo = require('./foo'); console.log(foo);"#);
        let transformed = transform(&module);
        assert!(
            has_import_decl(&transformed),
            "transformed module should have default import decl"
        );
    }

    #[test]
    fn transforms_module_exports() {
        let module = parse(r#"module.exports.foo = 42;"#);
        let transformed = transform(&module);
        assert!(
            has_let_decl(&transformed),
            "transformed module should have let decl"
        );
        assert!(
            has_default_export(&transformed),
            "transformed module should have synthetic default export"
        );
    }

    #[test]
    fn transforms_exports_alias() {
        let module = parse(r#"exports.bar = 42;"#);
        let transformed = transform(&module);
        assert!(
            has_let_decl(&transformed),
            "transformed module should have let decl"
        );
        assert!(
            has_default_export(&transformed),
            "transformed module should have synthetic default export"
        );
    }

    #[test]
    fn transforms_module_exports_default() {
        let module = parse(r#"module.exports = { foo: 1 };"#);
        let transformed = transform(&module);
        assert!(
            has_default_export(&transformed),
            "transformed module should have default export"
        );
    }

    #[test]
    fn require_direct_import_uses_user_var_name() {
        let module = parse(r#"const lib = require('./lib'); console.log(lib);"#);
        let transformed = transform(&module);
        assert!(
            has_import_with_local(&transformed, "lib"),
            "import should use user variable name 'lib'"
        );
        let has_const_lib = transformed.body.iter().any(|item| {
            if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) = item {
                var.decls.iter().any(|d| {
                    if let ast::Pat::Ident(b) = &d.name {
                        b.id.sym.as_ref() == "lib"
                    } else {
                        false
                    }
                })
            } else {
                false
            }
        });
        assert!(
            !has_const_lib,
            "should not have const lib = ... declaration"
        );
    }

    #[test]
    fn transform_preserves_module_decl_items() {
        let module = parse(r#"import { x } from './esm.js'; module.exports.foo = 1;"#);
        let transformed = transform(&module);
        let has_esm_import = transformed.body.iter().any(|item| {
            if let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import)) = item {
                import.specifiers.iter().any(|s| {
                    if let ast::ImportSpecifier::Named(n) = s {
                        n.local.sym.as_ref() == "x"
                    } else {
                        false
                    }
                })
            } else {
                false
            }
        });
        assert!(has_esm_import, "ESM import should be preserved");
    }

    #[test]
    fn transform_with_prefix_adds_prefix_to_var_names() {
        let module = parse(r#"module.exports.foo = 42;"#);
        let transformed = transform_with_prefix(&module, "_1_");
        let has_prefixed_var = transformed.body.iter().any(|item| {
            if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) = item {
                var.decls.iter().any(|d| {
                    if let ast::Pat::Ident(b) = &d.name {
                        b.id.sym.as_ref() == "_1___cjs_foo"
                    } else {
                        false
                    }
                })
            } else {
                false
            }
        });
        assert!(
            has_prefixed_var,
            "should have prefixed variable name _1___cjs_foo"
        );
    }

    #[test]
    fn transform_skips_synthetic_default_when_has_default() {
        let module = parse(r#"module.exports = { foo: 1 }; module.exports.bar = 2;"#);
        let transformed = transform(&module);
        let default_export_count = transformed
            .body
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDefaultExpr(_))
                )
            })
            .count();
        assert_eq!(
            default_export_count, 1,
            "should have exactly one default export"
        );
    }

    #[test]
    fn multiple_require_same_specifier_uses_first() {
        let module =
            parse(r#"const a = require('./foo'); const b = require('./foo'); console.log(a, b);"#);
        let transformed = transform(&module);
        let import_count = transformed
            .body
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(_))
                )
            })
            .count();
        assert_eq!(
            import_count, 1,
            "same specifier should produce only one import"
        );
    }

    #[test]
    fn require_in_non_var_context_generates_auto_name() {
        let module = parse(r#"console.log(require('./foo'));"#);
        let transformed = transform(&module);
        let has_auto_import = transformed.body.iter().any(|item| {
            if let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import)) = item {
                import.specifiers.iter().any(|s| {
                    if let ast::ImportSpecifier::Default(d) = s {
                        d.local.sym.as_ref().starts_with("__cjs_req_")
                    } else {
                        false
                    }
                })
            } else {
                false
            }
        });
        assert!(
            has_auto_import,
            "non-var require should generate __cjs_req_N import"
        );
    }

    #[test]
    fn non_assign_expr_stmt_not_transformed() {
        let module = parse(r#"console.log(1);"#);
        let transformed = transform(&module);
        let has_expr_stmt = transformed
            .body
            .iter()
            .any(|item| matches!(item, ast::ModuleItem::Stmt(ast::Stmt::Expr(_))));
        assert!(
            has_expr_stmt,
            "non-assign expression statement should be preserved"
        );
    }

    #[test]
    fn compound_assign_not_transformed() {
        let module = parse(r#"let x = 1; x += 2;"#);
        let transformed = transform(&module);
        assert!(
            !has_default_export(&transformed),
            "compound assignment should not produce default export"
        );
    }

    #[test]
    fn non_member_assign_not_transformed() {
        let module = parse(r#"let x = 1; x = 2;"#);
        let transformed = transform(&module);
        assert!(
            !has_default_export(&transformed),
            "simple assignment should not produce default export"
        );
    }

    #[test]
    fn computed_string_property_exports() {
        let module = parse(r#"exports['foo'] = 42;"#);
        let transformed = transform(&module);
        assert!(
            has_let_decl(&transformed),
            "computed string property should produce let decl"
        );
        assert!(
            has_default_export(&transformed),
            "computed string property should produce synthetic default export"
        );
    }

    #[test]
    fn computed_non_string_property_not_transformed() {
        let module = parse(r#"let key = 'foo'; exports[key] = 42;"#);
        let transformed = transform(&module);
        let cjs_let_count = transformed
            .body
            .iter()
            .filter(|item| {
                if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) = item {
                    var.decls.iter().any(|d| {
                        if let ast::Pat::Ident(b) = &d.name {
                            b.id.sym.as_ref().starts_with("__cjs_")
                        } else {
                            false
                        }
                    })
                } else {
                    false
                }
            })
            .count();
        assert_eq!(
            cjs_let_count, 0,
            "non-string computed property should not produce __cjs_ let decl"
        );
    }

    #[test]
    fn transform_expr_handles_binary() {
        let module = parse(r#"const x = require('./foo'); console.log(x + 1);"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_unary() {
        let module = parse(r#"const x = require('./foo'); console.log(-x);"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_update() {
        let module = parse(r#"let x = 1; x++; console.log(x);"#);
        let transformed = transform(&module);
        let has_var = transformed.body.iter().any(|item| {
            matches!(
                item,
                ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(_)))
            )
        });
        assert!(has_var);
    }

    #[test]
    fn transform_expr_handles_conditional() {
        let module = parse(r#"const x = require('./foo'); console.log(x ? 1 : 2);"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_sequence() {
        let module = parse(r#"const x = require('./foo'); console.log((x, 1));"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_arrow_with_body() {
        let module = parse(r#"const x = require('./foo'); const fn = () => { return x; };"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_arrow_expr_body() {
        let module = parse(r#"const x = require('./foo'); const fn = () => x;"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_template() {
        let module = parse(r#"const x = require('./foo'); console.log(`${x}`);"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_new() {
        let module = parse(r#"const x = require('./foo'); console.log(new x());"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_paren() {
        let module = parse(r#"const x = require('./foo'); console.log((x));"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_object_spread() {
        let module = parse(r#"const x = require('./foo'); console.log({ ...x });"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_opt_chain_member() {
        let module = parse(r#"const x = require('./foo'); console.log(x?.y);"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_opt_chain_call() {
        let module = parse(r#"const x = require('./foo'); console.log(x?.());"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_await() {
        let module = parse(r#"async function f() { const x = require('./foo'); await x; }"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_yield() {
        let module = parse(r#"function* f() { const x = require('./foo'); yield x; }"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_fn_expr() {
        let module = parse(r#"const x = require('./foo'); const fn = function() { return x; };"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_class_expr() {
        let module = parse(r#"const x = require('./foo'); const c = class { m() { return x; } };"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_tagged_template() {
        let module = parse(
            r#"const x = require('./foo'); function tag(t, v) { return v; } console.log(tag`${x}`);"#,
        );
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_stmt_handles_block() {
        let module = parse(r#"const x = require('./foo'); { console.log(x); }"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_stmt_handles_if() {
        let module = parse(r#"const x = require('./foo'); if (true) { console.log(x); }"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_stmt_handles_if_with_else() {
        let module = parse(
            r#"const x = require('./foo'); if (true) { console.log(x); } else { console.log(x); }"#,
        );
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_stmt_handles_while() {
        let module = parse(r#"const x = require('./foo'); while (false) { console.log(x); }"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_stmt_handles_for() {
        let module =
            parse(r#"const x = require('./foo'); for (let i = 0; i < 1; i++) { console.log(x); }"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_stmt_handles_switch() {
        let module =
            parse(r#"const x = require('./foo'); switch (1) { case 1: console.log(x); break; }"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_stmt_handles_try_catch() {
        let module = parse(
            r#"const x = require('./foo'); try { console.log(x); } catch (e) { console.log(e); }"#,
        );
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_stmt_handles_try_finally() {
        let module = parse(
            r#"const x = require('./foo'); try { console.log(x); } finally { console.log(x); }"#,
        );
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_stmt_handles_return() {
        let module = parse(r#"function f() { const x = require('./foo'); return x; }"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_stmt_handles_throw() {
        let module = parse(r#"const x = require('./foo'); throw x;"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_stmt_handles_labeled() {
        let module = parse(r#"const x = require('./foo'); label: { console.log(x); }"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn var_decl_with_non_require_init_preserved() {
        let module = parse(r#"const x = 42; module.exports.foo = x;"#);
        let transformed = transform(&module);
        let has_const_x = transformed.body.iter().any(|item| {
            if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) = item {
                var.decls.iter().any(|d| {
                    if let ast::Pat::Ident(b) = &d.name {
                        b.id.sym.as_ref() == "x"
                    } else {
                        false
                    }
                })
            } else {
                false
            }
        });
        assert!(has_const_x, "non-require var decl should be preserved");
    }

    #[test]
    fn is_module_exports_member_returns_false_for_non_member() {
        assert!(!is_module_exports_member(&ast::Expr::Ident(
            ast::Ident::new("module".into(), DUMMY_SP, SyntaxContext::default(),)
        )));
    }

    #[test]
    fn is_exports_member_returns_false_for_non_member() {
        assert!(!is_exports_member(&ast::Expr::Ident(ast::Ident::new(
            "exports".into(),
            DUMMY_SP,
            SyntaxContext::default(),
        ))));
    }

    #[test]
    fn is_exports_ident_returns_false_for_non_ident() {
        assert!(!is_exports_ident(&ast::Expr::Lit(ast::Lit::Num(
            ast::Number {
                span: DUMMY_SP,
                value: 1.0,
                raw: None,
            }
        ))));
    }

    #[test]
    fn is_module_exports_member_no_prop_returns_false_for_non_ident_obj() {
        let obj = ast::Expr::Lit(ast::Lit::Null(ast::Null { span: DUMMY_SP }));
        let prop = ast::MemberProp::Ident(ast::IdentName::new("exports".into(), DUMMY_SP));
        assert!(!is_module_exports_member_no_prop(&obj, &prop));
    }

    #[test]
    fn is_module_exports_member_no_prop_returns_false_for_non_ident_prop() {
        let obj = ast::Expr::Ident(ast::Ident::new(
            "module".into(),
            DUMMY_SP,
            SyntaxContext::default(),
        ));
        let prop = ast::MemberProp::Computed(ast::ComputedPropName {
            span: DUMMY_SP,
            expr: Box::new(ast::Expr::Lit(ast::Lit::Str(ast::Str {
                span: DUMMY_SP,
                value: "exports".into(),
                raw: None,
            }))),
        });
        assert!(!is_module_exports_member_no_prop(&obj, &prop));
    }

    #[test]
    fn is_module_exports_member_returns_false_for_wrong_obj_name() {
        let module = parse(r#"obj.exports.foo = 1;"#);
        assert!(!is_commonjs_module(&module));
    }

    #[test]
    fn is_module_exports_member_returns_false_for_wrong_prop_name() {
        let module = parse(r#"module.other.foo = 1;"#);
        assert!(!is_commonjs_module(&module));
    }

    #[test]
    fn transform_expr_handles_assign_pat_target() {
        let module = parse(r#"let x; ({x} = {x: 1}); console.log(x);"#);
        let transformed = transform(&module);
        let has_expr = transformed
            .body
            .iter()
            .any(|item| matches!(item, ast::ModuleItem::Stmt(ast::Stmt::Expr(_))));
        assert!(has_expr);
    }

    #[test]
    fn transform_expr_handles_assign_simple_non_member() {
        let module = parse(r#"let x = 1; x = 2; console.log(x);"#);
        let transformed = transform(&module);
        assert!(!has_default_export(&transformed));
    }

    #[test]
    fn transform_decl_handles_non_var() {
        let module = parse(r#"function foo() {} module.exports.bar = 1;"#);
        let transformed = transform(&module);
        let has_fn_decl = transformed.body.iter().any(|item| {
            matches!(
                item,
                ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Fn(_)))
            )
        });
        assert!(has_fn_decl, "function declaration should be preserved");
    }

    #[test]
    fn transform_var_decl_empty_after_removal() {
        let module = parse(r#"const lib = require('./lib'); console.log(lib);"#);
        let transformed = transform(&module);
        let has_empty_const = transformed.body.iter().any(|item| {
            if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) = item {
                var.kind == ast::VarDeclKind::Const && var.decls.is_empty()
            } else {
                false
            }
        });
        assert!(!has_empty_const, "empty var decls should be removed");
    }

    #[test]
    fn transform_block_handles_export_decl_in_block() {
        let module = parse(r#"const x = require('./foo'); { console.log(x); }"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_stmt_handles_do_while() {
        let module = parse(r#"const x = require('./foo'); do { console.log(x); } while (false);"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_stmt_handles_for_in() {
        let module = parse(r#"const x = require('./foo'); for (let k in {}) { console.log(x); }"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_stmt_handles_for_of() {
        let module = parse(r#"const x = require('./foo'); for (let v of []) { console.log(x); }"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_stmt_handles_with() {
        let module = parse(r#"const x = require('./foo'); with ({}) { console.log(x); }"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_member() {
        let module = parse(r#"const x = require('./foo'); console.log(x.y);"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_assign_member() {
        let module = parse(r#"const x = require('./foo'); x.y = 1;"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_array() {
        let module = parse(r#"const x = require('./foo'); console.log([x]);"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_object() {
        let module = parse(r#"const x = require('./foo'); console.log({ y: x });"#);
        let transformed = transform(&module);
        assert!(has_import_decl(&transformed));
    }

    #[test]
    fn transform_expr_handles_call_with_super() {
        let module = parse(r#"class A { constructor() { super(); } }"#);
        let transformed = transform(&module);
        let has_class = transformed.body.iter().any(|item| {
            matches!(
                item,
                ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Class(_)))
            )
        });
        assert!(has_class);
    }

    // ========== 修复验证测试 ==========

    /// 测试 require() 在函数表达式体内被正确处理
    /// 预期行为：const x = require('./foo') 被移除，x 引用导入的标识符
    #[test]
    fn require_in_fn_expr_body_is_transformed() {
        let module = parse(
            r#"
            const fn = function() {
                const x = require('./foo');
                return x;
            };
        "#,
        );
        let transformed = transform(&module);

        // 应该有 import 声明
        assert!(
            has_import_decl(&transformed),
            "should have import declaration"
        );

        // 验证函数体内没有 require() 调用（var decl 被移除或转换）
        let fn_body_ok = transformed.body.iter().any(|item| {
            if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) = item {
                for decl in &var.decls {
                    if let ast::Pat::Ident(binding) = &decl.name {
                        if binding.id.sym == "fn" {
                            if let Some(ast::Expr::Fn(f)) = decl.init.as_deref() {
                                if let Some(body) = &f.function.body {
                                    // 检查函数体内是否有 var decl
                                    for stmt in &body.stmts {
                                        if let ast::Stmt::Decl(ast::Decl::Var(v)) = stmt {
                                            for d in &v.decls {
                                                if let ast::Pat::Ident(b) = &d.name {
                                                    if b.id.sym == "x" {
                                                        // 如果有 const x = ...，检查初始化器不是 require()
                                                        if let Some(init) = &d.init {
                                                            if let ast::Expr::Call(call) =
                                                                init.as_ref()
                                                            {
                                                                if let ast::Callee::Expr(callee) =
                                                                    &call.callee
                                                                {
                                                                    if let ast::Expr::Ident(id) =
                                                                        callee.as_ref()
                                                                    {
                                                                        if id.sym == "require" {
                                                                            return false; // require() 未被转换
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    return true; // 没有 require() 调用
                                }
                            }
                        }
                    }
                }
            }
            false
        });
        assert!(
            fn_body_ok,
            "require() in function expression body should be transformed"
        );
    }

    /// 测试 require() 在函数声明体内被正确处理
    /// 预期行为：const x = require('./foo') 被移除，x 引用导入的标识符
    #[test]
    fn require_in_fn_decl_body_is_transformed() {
        let module = parse(
            r#"
            function fn() {
                const x = require('./foo');
                return x;
            }
        "#,
        );
        let transformed = transform(&module);
        assert!(
            has_import_decl(&transformed),
            "should have import declaration"
        );

        // 验证函数声明体内没有 require() 调用
        let fn_body_ok = transformed.body.iter().any(|item| {
            if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Fn(fn_decl))) = item {
                if fn_decl.ident.sym == "fn" {
                    if let Some(body) = &fn_decl.function.body {
                        for stmt in &body.stmts {
                            if let ast::Stmt::Decl(ast::Decl::Var(v)) = stmt {
                                for d in &v.decls {
                                    if let ast::Pat::Ident(b) = &d.name {
                                        if b.id.sym == "x" {
                                            if let Some(init) = &d.init {
                                                if let ast::Expr::Call(call) = init.as_ref() {
                                                    if let ast::Callee::Expr(callee) = &call.callee
                                                    {
                                                        if let ast::Expr::Ident(id) =
                                                            callee.as_ref()
                                                        {
                                                            if id.sym == "require" {
                                                                return false;
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        return true;
                    }
                }
            }
            false
        });
        assert!(
            fn_body_ok,
            "require() in function declaration body should be transformed"
        );
    }

    /// 测试 require() 在类方法体内被正确处理
    /// 预期行为：const x = require('./foo') 被移除，x 引用导入的标识符
    #[test]
    fn require_in_class_method_body_is_transformed() {
        let module = parse(
            r#"
            class MyClass {
                method() {
                    const x = require('./foo');
                    return x;
                }
            }
        "#,
        );
        let transformed = transform(&module);
        assert!(
            has_import_decl(&transformed),
            "should have import declaration"
        );

        // 验证类方法体内没有 require() 调用
        let method_body_ok = transformed.body.iter().any(|item| {
            if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Class(class_decl))) = item {
                if class_decl.ident.sym == "MyClass" {
                    for member in &class_decl.class.body {
                        if let ast::ClassMember::Method(method) = member {
                            if let Some(body) = &method.function.body {
                                for stmt in &body.stmts {
                                    if let ast::Stmt::Decl(ast::Decl::Var(v)) = stmt {
                                        for d in &v.decls {
                                            if let ast::Pat::Ident(b) = &d.name {
                                                if b.id.sym == "x" {
                                                    if let Some(init) = &d.init {
                                                        if let ast::Expr::Call(call) = init.as_ref()
                                                        {
                                                            if let ast::Callee::Expr(callee) =
                                                                &call.callee
                                                            {
                                                                if let ast::Expr::Ident(id) =
                                                                    callee.as_ref()
                                                                {
                                                                    if id.sym == "require" {
                                                                        return false;
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                return true;
                            }
                        }
                    }
                }
            }
            false
        });
        assert!(
            method_body_ok,
            "require() in class method body should be transformed"
        );
    }

    /// 测试 module.exports = X 后 module.exports.y = Z 同时导出两者
    #[test]
    fn module_exports_default_and_named_both_exported() {
        let module = parse(
            r#"
            module.exports = function() { return 42; };
            module.exports.VERSION = '1.0';
        "#,
        );
        let transformed = transform(&module);

        // 应该有默认导出
        let has_default = transformed.body.iter().any(|item| {
            matches!(
                item,
                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDefaultExpr(_))
            )
        });
        assert!(has_default, "should have default export");

        // 应该有命名导出（VERSION）
        let has_named = transformed.body.iter().any(|item| {
            if let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportNamed(named)) = item {
                named.specifiers.iter().any(|s| {
                    if let ast::ExportSpecifier::Named(n) = s {
                        n.exported
                            .as_ref()
                            .map(|e| {
                                if let ast::ModuleExportName::Ident(id) = e {
                                    id.sym == "VERSION"
                                } else {
                                    false
                                }
                            })
                            .unwrap_or(false)
                    } else {
                        false
                    }
                })
            } else {
                false
            }
        });
        assert!(has_named, "should have named export for VERSION");

        // VERSION 变量应该存在
        let has_version_var = transformed.body.iter().any(|item| {
            if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) = item {
                var.decls.iter().any(|d| {
                    if let ast::Pat::Ident(b) = &d.name {
                        b.id.sym.contains("VERSION")
                    } else {
                        false
                    }
                })
            } else {
                false
            }
        });
        assert!(has_version_var, "VERSION variable should exist");
    }
}
