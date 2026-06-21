use swc_core::ecma::ast as swc_ast;

/// 检测模块体是否包含 top-level `await`（不递归进入函数/类体边界）
pub(crate) fn has_top_level_await(module: &swc_ast::Module) -> bool {
    fn expr_has_await(expr: &swc_ast::Expr) -> bool {
        match expr {
            swc_ast::Expr::Await(_) => true,
            // 边界：不递归进入函数/类体
            swc_ast::Expr::Fn(_) | swc_ast::Expr::Arrow(_) | swc_ast::Expr::Class(_) => false,
            // 递归检查子表达式
            swc_ast::Expr::Array(a) => a
                .elems
                .iter()
                .any(|e| e.as_ref().is_some_and(|e| expr_has_await(&e.expr))),
            swc_ast::Expr::Object(o) => o.props.iter().any(|p| match p {
                swc_ast::PropOrSpread::Spread(s) => expr_has_await(&s.expr),
                swc_ast::PropOrSpread::Prop(p) => match &**p {
                    swc_ast::Prop::KeyValue(kv) => expr_has_await(&kv.value),
                    swc_ast::Prop::Assign(a) => expr_has_await(&a.value),
                    _ => false,
                },
            }),
            swc_ast::Expr::Unary(u) => expr_has_await(&u.arg),
            swc_ast::Expr::Update(u) => expr_has_await(&u.arg),
            swc_ast::Expr::Bin(b) => expr_has_await(&b.left) || expr_has_await(&b.right),
            swc_ast::Expr::Assign(a) => expr_has_await(&a.right),
            swc_ast::Expr::Member(m) => {
                expr_has_await(&m.obj)
                    || matches!(&m.prop, swc_ast::MemberProp::Computed(c) if expr_has_await(&c.expr))
            }
            swc_ast::Expr::Cond(c) => {
                expr_has_await(&c.test) || expr_has_await(&c.cons) || expr_has_await(&c.alt)
            }
            swc_ast::Expr::Call(c) => {
                (match &c.callee {
                    swc_ast::Callee::Expr(e) => expr_has_await(e),
                    _ => false,
                }) || c.args.iter().any(|a| expr_has_await(&a.expr))
            }
            swc_ast::Expr::New(n) => {
                expr_has_await(&n.callee)
                    || n.args
                        .as_ref()
                        .is_some_and(|a| a.iter().any(|a| expr_has_await(&a.expr)))
            }
            swc_ast::Expr::Seq(s) => s.exprs.iter().any(|e| expr_has_await(e)),
            swc_ast::Expr::Tpl(t) => t.exprs.iter().any(|e| expr_has_await(e)),
            swc_ast::Expr::TaggedTpl(t) => {
                expr_has_await(&t.tag) || t.tpl.exprs.iter().any(|e| expr_has_await(e))
            }
            swc_ast::Expr::Yield(y) => y.arg.as_ref().is_some_and(|a| expr_has_await(a)),
            swc_ast::Expr::Paren(p) => expr_has_await(&p.expr),
            _ => false,
        }
    }

    fn decl_has_await(decl: &swc_ast::Decl) -> bool {
        match decl {
            swc_ast::Decl::Var(v) => v
                .decls
                .iter()
                .any(|d| d.init.as_ref().is_some_and(|i| expr_has_await(i))),
            swc_ast::Decl::Fn(_) | swc_ast::Decl::Class(_) => false,
            _ => false,
        }
    }

    fn stmt_has_await(stmt: &swc_ast::Stmt) -> bool {
        match stmt {
            swc_ast::Stmt::Expr(e) => expr_has_await(&e.expr),
            swc_ast::Stmt::Decl(d) => decl_has_await(d),
            swc_ast::Stmt::Block(b) => b.stmts.iter().any(stmt_has_await),
            swc_ast::Stmt::If(i) => {
                expr_has_await(&i.test)
                    || stmt_has_await(&i.cons)
                    || i.alt.as_ref().is_some_and(|a| stmt_has_await(a))
            }
            swc_ast::Stmt::While(w) => expr_has_await(&w.test) || stmt_has_await(&w.body),
            swc_ast::Stmt::DoWhile(d) => expr_has_await(&d.test) || stmt_has_await(&d.body),
            swc_ast::Stmt::For(f) => {
                f.init.as_ref().is_some_and(|init| match init {
                    swc_ast::VarDeclOrExpr::VarDecl(v) => v
                        .decls
                        .iter()
                        .any(|d| d.init.as_ref().is_some_and(|i| expr_has_await(i))),
                    swc_ast::VarDeclOrExpr::Expr(e) => expr_has_await(e),
                }) || f.test.as_ref().is_some_and(|t| expr_has_await(t))
                    || f.update.as_ref().is_some_and(|u| expr_has_await(u))
                    || stmt_has_await(&f.body)
            }
            swc_ast::Stmt::ForIn(f) => expr_has_await(&f.right) || stmt_has_await(&f.body),
            swc_ast::Stmt::ForOf(f) => {
                f.is_await || expr_has_await(&f.right) || stmt_has_await(&f.body)
            }
            swc_ast::Stmt::Return(r) => r.arg.as_ref().is_some_and(|a| expr_has_await(a)),
            swc_ast::Stmt::Throw(t) => expr_has_await(&t.arg),
            swc_ast::Stmt::Try(t) => {
                t.block.stmts.iter().any(stmt_has_await)
                    || t.handler
                        .as_ref()
                        .is_some_and(|h| h.body.stmts.iter().any(stmt_has_await))
                    || t.finalizer
                        .as_ref()
                        .is_some_and(|f| f.stmts.iter().any(stmt_has_await))
            }
            swc_ast::Stmt::Switch(s) => {
                expr_has_await(&s.discriminant)
                    || s.cases.iter().any(|c| {
                        c.test.as_ref().is_some_and(|t| expr_has_await(t))
                            || c.cons.iter().any(stmt_has_await)
                    })
            }
            swc_ast::Stmt::Labeled(l) => stmt_has_await(&l.body),
            swc_ast::Stmt::With(w) => expr_has_await(&w.obj) || stmt_has_await(&w.body),
            _ => false,
        }
    }

    for item in &module.body {
        match item {
            swc_ast::ModuleItem::Stmt(stmt) => {
                if stmt_has_await(stmt) {
                    return true;
                }
            }
            swc_ast::ModuleItem::ModuleDecl(decl) => match decl {
                swc_ast::ModuleDecl::ExportDecl(e) => {
                    if decl_has_await(&e.decl) {
                        return true;
                    }
                }
                swc_ast::ModuleDecl::ExportDefaultExpr(e) if expr_has_await(&e.expr) => {
                    return true;
                }
                _ => {}
            },
        }
    }
    false
}
