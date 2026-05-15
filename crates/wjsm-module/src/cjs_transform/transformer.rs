use std::collections::{BTreeMap, HashSet};

use swc_core::common::{DUMMY_SP, SyntaxContext};
use swc_core::ecma::ast;

use super::helpers::{
    create_import_default_decl, create_let_decl, extract_require_specifier,
    is_exports_ident, is_module_exports_member, is_module_exports_member_no_prop,
};

pub(super) struct CjsTransformer {
    pub(super) require_map: BTreeMap<String, String>,
    pub(super) direct_imports: HashSet<String>,
    pub(super) export_names: Vec<(String, String)>,
    pub(super) has_default_export: bool,
    pub(super) export_prefix: String,
}

impl CjsTransformer {
    pub(super) fn transform_module(&mut self, module: &ast::Module) -> ast::Module {
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

    fn transform_function(&mut self, func: &ast::Function) -> ast::Function {
        let mut new_func = func.clone();
        if let Some(body) = new_func.body.take() {
            new_func.body = Some(self.transform_block(&body));
        }
        new_func
    }

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
                let mut new_fn_decl = fn_decl.clone();
                new_fn_decl.function = Box::new(self.transform_function(&fn_decl.function));
                ast::Stmt::Decl(ast::Decl::Fn(new_fn_decl))
            }
            ast::Decl::Class(class_decl) => {
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
