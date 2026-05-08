// 模块解析器：文件系统模块解析、import/export 提取

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
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
    Named {
        local: String,
        exported: String,
    },
    /// export default expr
    Default {
        local: String, // 内部生成的变量名
    },
    /// export * from './foo'
    All {
        source: String,
    },
    /// export const/let/var/function/class
    Declaration {
        name: String,
    },
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
        Self {
            root_path: root_path.to_path_buf(),
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
        let mut ast = if crate::cjs_transform::is_commonjs_module(&ast) {
            // 使用模块路径的哈希作为前缀，避免多个 CJS 模块的变量名冲突
            let prefix = format!("_{}_", self.next_id);
            crate::cjs_transform::transform_with_prefix(&ast, &prefix)
        } else {
            ast
        };

        // 提取 import/export
        let imports = Self::extract_imports(&ast);
        let mut exports = Self::extract_exports(&ast);

        // 如果模块没有默认导出但有其他导出，添加合成默认导出
        // 这样 CJS 模块通过 require() 导入时可以使用默认导入
        let has_default_export = exports.iter().any(|e| matches!(e, ExportEntry::Default { .. }));
        if !has_default_export && !exports.is_empty() {
            // 为模块添加合成默认导出
            Self::add_synthetic_default_export(&mut ast, &exports);
            // 重新提取 exports
            exports = Self::extract_exports(&ast);
        }

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

    /// 为没有默认导出的模块添加合成默认导出
    fn add_synthetic_default_export(ast: &mut ast::Module, exports: &[ExportEntry]) {
        use swc_core::common::{DUMMY_SP, SyntaxContext};

        // 收集所有命名导出的名称
        let mut export_names: Vec<String> = Vec::new();
        for entry in exports {
            match entry {
                ExportEntry::Named { exported, .. } => {
                    if exported != "default" && !export_names.contains(exported) {
                        export_names.push(exported.clone());
                    }
                }
                ExportEntry::Declaration { name } => {
                    if !export_names.contains(name) {
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
                                        ast::ModuleExportName::Ident(ident) => ident.sym.to_string(),
                                        ast::ModuleExportName::Str(s) => s.value.to_string_lossy().into_owned(),
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

    /// 从 AST 中提取 export 声明
    fn extract_exports(module: &ast::Module) -> Vec<ExportEntry> {
        let mut exports = Vec::new();

        for item in &module.body {
            match item {
                ast::ModuleItem::ModuleDecl(decl) => match decl {
                    ast::ModuleDecl::ExportNamed(named_export) => {
                        if let Some(src) = &named_export.src {
                            // export { ... } from './foo'
                            exports.push(ExportEntry::All {
                                source: src.value.to_string_lossy().into_owned(),
                            });
                        } else {
                            // export { x } / export { x as y }
                            for spec in &named_export.specifiers {
                                match spec {
                                    ast::ExportSpecifier::Named(named) => {
                                        let local = match &named.orig {
                                            ast::ModuleExportName::Ident(ident) => {
                                                ident.sym.to_string()
                                            }
                                            ast::ModuleExportName::Str(s) => s.value.to_string_lossy().into_owned(),
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
                    ast::ModuleDecl::ExportDefaultExpr(default_expr) => {
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
