use swc_core::ecma::ast as swc_ast;

#[allow(dead_code)]
pub(crate) fn stmt_kind(stmt: &swc_ast::Stmt) -> &'static str {
    match stmt {
        swc_ast::Stmt::Block(_) => "block",
        swc_ast::Stmt::Empty(_) => "empty",
        swc_ast::Stmt::Debugger(_) => "debugger",
        swc_ast::Stmt::With(_) => "with",
        swc_ast::Stmt::Return(_) => "return",
        swc_ast::Stmt::Labeled(_) => "labeled",
        swc_ast::Stmt::Break(_) => "break",
        swc_ast::Stmt::Continue(_) => "continue",
        swc_ast::Stmt::If(_) => "if",
        swc_ast::Stmt::Switch(_) => "switch",
        swc_ast::Stmt::Throw(_) => "throw",
        swc_ast::Stmt::Try(_) => "try",
        swc_ast::Stmt::While(_) => "while",
        swc_ast::Stmt::DoWhile(_) => "do-while",
        swc_ast::Stmt::For(_) => "for",
        swc_ast::Stmt::ForIn(_) => "for-in",
        swc_ast::Stmt::ForOf(_) => "for-of",
        swc_ast::Stmt::Decl(_) => "decl",
        swc_ast::Stmt::Expr(_) => "expr",
    }
}

pub(crate) fn decl_kind(decl: &swc_ast::Decl) -> &'static str {
    match decl {
        swc_ast::Decl::Class(_) => "class",
        swc_ast::Decl::Fn(_) => "function",
        swc_ast::Decl::Var(_) => "var",
        swc_ast::Decl::Using(_) => "using",
        swc_ast::Decl::TsInterface(_) => "ts-interface",
        swc_ast::Decl::TsTypeAlias(_) => "ts-type-alias",
        swc_ast::Decl::TsEnum(_) => "ts-enum",
        swc_ast::Decl::TsModule(_) => "ts-module",
    }
}

pub(crate) fn expr_kind(expr: &swc_ast::Expr) -> &'static str {
    match expr {
        swc_ast::Expr::This(_) => "this",
        swc_ast::Expr::Array(_) => "array",
        swc_ast::Expr::Object(_) => "object",
        swc_ast::Expr::Fn(_) => "function",
        swc_ast::Expr::Unary(_) => "unary",
        swc_ast::Expr::Update(_) => "update",
        swc_ast::Expr::Bin(_) => "binary",
        swc_ast::Expr::Assign(_) => "assign",
        swc_ast::Expr::Member(_) => "member",
        swc_ast::Expr::SuperProp(_) => "super-prop",
        swc_ast::Expr::Cond(_) => "conditional",
        swc_ast::Expr::Call(_) => "call",
        swc_ast::Expr::New(_) => "new",
        swc_ast::Expr::Seq(_) => "sequence",
        swc_ast::Expr::Ident(_) => "identifier",
        swc_ast::Expr::Lit(_) => "literal",
        swc_ast::Expr::Tpl(_) => "template",
        swc_ast::Expr::TaggedTpl(_) => "tagged-template",
        swc_ast::Expr::Arrow(_) => "arrow",
        swc_ast::Expr::Class(_) => "class",
        swc_ast::Expr::Yield(_) => "yield",
        swc_ast::Expr::MetaProp(_) => "meta-prop",
        swc_ast::Expr::Await(_) => "await",
        swc_ast::Expr::Paren(_) => "paren",
        swc_ast::Expr::JSXMember(_) => "jsx-member",
        swc_ast::Expr::JSXNamespacedName(_) => "jsx-namespaced-name",
        swc_ast::Expr::JSXEmpty(_) => "jsx-empty",
        swc_ast::Expr::JSXElement(_) => "jsx-element",
        swc_ast::Expr::JSXFragment(_) => "jsx-fragment",
        swc_ast::Expr::TsTypeAssertion(_) => "ts-type-assertion",
        swc_ast::Expr::TsConstAssertion(_) => "ts-const-assertion",
        swc_ast::Expr::TsNonNull(_) => "ts-non-null",
        swc_ast::Expr::TsAs(_) => "ts-as",
        swc_ast::Expr::TsInstantiation(_) => "ts-instantiation",
        swc_ast::Expr::TsSatisfies(_) => "ts-satisfies",
        swc_ast::Expr::PrivateName(_) => "private-name",
        swc_ast::Expr::OptChain(_) => "optional-chain",
        swc_ast::Expr::Invalid(_) => "invalid",
    }
}

pub(crate) fn literal_kind(lit: &swc_ast::Lit) -> &'static str {
    match lit {
        swc_ast::Lit::Str(_) => "string",
        swc_ast::Lit::Bool(_) => "bool",
        swc_ast::Lit::Null(_) => "null",
        swc_ast::Lit::Num(_) => "number",
        swc_ast::Lit::BigInt(_) => "bigint",
        swc_ast::Lit::Regex(_) => "regex",
        swc_ast::Lit::JSXText(_) => "jsx-text",
    }
}

#[allow(dead_code)]
pub(crate) fn module_decl_kind(decl: &swc_ast::ModuleDecl) -> &'static str {
    match decl {
        swc_ast::ModuleDecl::Import(_) => "import",
        swc_ast::ModuleDecl::ExportDecl(_) => "export-decl",
        swc_ast::ModuleDecl::ExportNamed(_) => "export-named",
        swc_ast::ModuleDecl::ExportDefaultDecl(_) => "export-default-decl",
        swc_ast::ModuleDecl::ExportDefaultExpr(_) => "export-default-expr",
        swc_ast::ModuleDecl::ExportAll(_) => "export-all",
        swc_ast::ModuleDecl::TsImportEquals(_) => "ts-import-equals",
        swc_ast::ModuleDecl::TsExportAssignment(_) => "ts-export-assignment",
        swc_ast::ModuleDecl::TsNamespaceExport(_) => "ts-namespace-export",
    }
}

pub(crate) fn decl_exported_names(decl: &swc_ast::Decl) -> Vec<String> {
    match decl {
        swc_ast::Decl::Var(var_decl) => {
            var_decl
                .decls
                .iter()
                .map(|d| {
                    match &d.name {
                        swc_ast::Pat::Ident(ident) => ident.id.sym.to_string(),
                        _ => String::new(),
                    }
                })
                .filter(|s| !s.is_empty())
                .collect()
        }
        swc_ast::Decl::Fn(fn_decl) => {
            vec![fn_decl.ident.sym.to_string()]
        }
        swc_ast::Decl::Class(class_decl) => {
            vec![class_decl.ident.sym.to_string()]
        }
        swc_ast::Decl::TsInterface(ts_iface) => {
            vec![ts_iface.id.sym.to_string()]
        }
        swc_ast::Decl::TsTypeAlias(ts_alias) => {
            vec![ts_alias.id.sym.to_string()]
        }
        swc_ast::Decl::TsEnum(ts_enum) => {
            vec![ts_enum.id.sym.to_string()]
        }
        swc_ast::Decl::TsModule(ts_module) => match &ts_module.id {
            swc_ast::TsModuleName::Ident(ident) => vec![ident.sym.to_string()],
            _ => vec![],
        },
        _ => vec![],
    }
}
