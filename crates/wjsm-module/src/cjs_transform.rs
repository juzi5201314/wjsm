// CJS AST 转换器：将 CommonJS 模块转换为 ESM 风格 AST
//
// 转换规则：
// 1. require('./path') → import __cjs_req_N from './path'; 使用 __cjs_req_N 替换
// 2. module.exports.x = value → export let x = value
// 3. exports.x = value → export let x = value
// 4. module.exports = obj → export default obj
// 5. 对于带有命名导出的 CJS 模块，生成合成默认导出：export default { x, y, ... }

use std::collections::HashMap;
use swc_core::common::{DUMMY_SP, SyntaxContext};
use swc_core::ecma::ast;

/// 检测模块是否包含 CommonJS 语法
pub fn is_commonjs_module(module: &ast::Module) -> bool {
    let mut detector = CjsDetector { found: false };
    for item in &module.body {
        detector.visit_module_item(item);
        if detector.found {
            return true;
        }
    }
    detector.found
}

struct CjsDetector {
    found: bool,
}

impl CjsDetector {
    fn visit_module_item(&mut self, item: &ast::ModuleItem) {
        if self.found {
            return;
        }
        match item {
            ast::ModuleItem::Stmt(stmt) => self.visit_stmt(stmt),
            ast::ModuleItem::ModuleDecl(decl) => self.visit_module_decl(decl),
        }
    }

    fn visit_module_decl(&mut self, decl: &ast::ModuleDecl) {
        match decl {
            ast::ModuleDecl::ExportDecl(export_decl) => {
                if let ast::Decl::Var(var_decl) = &export_decl.decl {
                    for decl in &var_decl.decls {
                        if let Some(init) = &decl.init {
                            self.visit_expr(init);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn visit_stmt(&mut self, stmt: &ast::Stmt) {
        if self.found {
            return;
        }
        match stmt {
            ast::Stmt::Expr(expr_stmt) => self.visit_expr(&expr_stmt.expr),
            ast::Stmt::Decl(decl) => self.visit_decl(decl),
            ast::Stmt::Block(block) => {
                for stmt in &block.stmts {
                    self.visit_stmt(stmt);
                }
            }
            ast::Stmt::If(if_stmt) => {
                self.visit_expr(&if_stmt.test);
                self.visit_stmt(&if_stmt.cons);
                if let Some(alt) = &if_stmt.alt {
                    self.visit_stmt(alt);
                }
            }
            ast::Stmt::While(while_stmt) => {
                self.visit_expr(&while_stmt.test);
                self.visit_stmt(&while_stmt.body);
            }
            ast::Stmt::DoWhile(do_while) => {
                self.visit_stmt(&do_while.body);
                self.visit_expr(&do_while.test);
            }
            ast::Stmt::For(for_stmt) => {
                if let Some(init) = &for_stmt.init {
                    match init {
                        ast::VarDeclOrExpr::VarDecl(var_decl) => self.visit_var_decl(var_decl),
                        ast::VarDeclOrExpr::Expr(expr) => self.visit_expr(expr),
                    }
                }
                if let Some(test) = &for_stmt.test {
                    self.visit_expr(test);
                }
                if let Some(update) = &for_stmt.update {
                    self.visit_expr(update);
                }
                self.visit_stmt(&for_stmt.body);
            }
            ast::Stmt::ForIn(for_in) => {
                self.visit_expr(&for_in.right);
                self.visit_stmt(&for_in.body);
            }
            ast::Stmt::ForOf(for_of) => {
                self.visit_expr(&for_of.right);
                self.visit_stmt(&for_of.body);
            }
            ast::Stmt::Switch(switch) => {
                self.visit_expr(&switch.discriminant);
                for case in &switch.cases {
                    if let Some(test) = &case.test {
                        self.visit_expr(test);
                    }
                    for stmt in &case.cons {
                        self.visit_stmt(stmt);
                    }
                }
            }
            ast::Stmt::Try(try_stmt) => {
                for stmt in &try_stmt.block.stmts {
                    self.visit_stmt(stmt);
                }
                if let Some(handler) = &try_stmt.handler {
                    for stmt in &handler.body.stmts {
                        self.visit_stmt(stmt);
                    }
                }
                if let Some(finalizer) = &try_stmt.finalizer {
                    for stmt in &finalizer.stmts {
                        self.visit_stmt(stmt);
                    }
                }
            }
            ast::Stmt::Labeled(labeled) => self.visit_stmt(&labeled.body),
            ast::Stmt::Return(ret) => {
                if let Some(arg) = &ret.arg {
                    self.visit_expr(arg);
                }
            }
            ast::Stmt::Throw(throw) => self.visit_expr(&throw.arg),
            ast::Stmt::With(with) => {
                self.visit_expr(&with.obj);
                self.visit_stmt(&with.body);
            }
            _ => {}
        }
    }

    fn visit_decl(&mut self, decl: &ast::Decl) {
        match decl {
            ast::Decl::Var(var_decl) => self.visit_var_decl(var_decl),
            ast::Decl::Fn(fn_decl) => {
                if let Some(body) = &fn_decl.function.body {
                    for stmt in &body.stmts {
                        self.visit_stmt(stmt);
                    }
                }
            }
            ast::Decl::Class(class_decl) => {
                if let Some(super_class) = &class_decl.class.super_class {
                    self.visit_expr(super_class);
                }
                for member in &class_decl.class.body {
                    match member {
                        ast::ClassMember::Method(method) => {
                            if let Some(body) = &method.function.body {
                                for stmt in &body.stmts {
                                    self.visit_stmt(stmt);
                                }
                            }
                        }
                        ast::ClassMember::PrivateMethod(method) => {
                            if let Some(body) = &method.function.body {
                                for stmt in &body.stmts {
                                    self.visit_stmt(stmt);
                                }
                            }
                        }
                        ast::ClassMember::Constructor(ctor) => {
                            if let Some(body) = &ctor.body {
                                for stmt in &body.stmts {
                                    self.visit_stmt(stmt);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn visit_var_decl(&mut self, var_decl: &ast::VarDecl) {
        for decl in &var_decl.decls {
            if let Some(init) = &decl.init {
                self.visit_expr(init);
            }
        }
    }

    fn visit_expr(&mut self, expr: &ast::Expr) {
        if self.found {
            return;
        }
        match expr {
            ast::Expr::Call(call) => {
                if is_require_call(call) {
                    self.found = true;
                    return;
                }
                for arg in &call.args {
                    self.visit_expr(&arg.expr);
                }
                if let ast::Callee::Expr(callee) = &call.callee {
                    self.visit_expr(callee);
                }
            }
            ast::Expr::Member(member) => {
                if is_module_exports_member(expr) || is_exports_member(expr) {
                    self.found = true;
                    return;
                }
                self.visit_expr(&member.obj);
                if let ast::MemberProp::Computed(computed) = &member.prop {
                    self.visit_expr(&computed.expr);
                }
            }
            ast::Expr::Bin(bin) => {
                self.visit_expr(&bin.left);
                self.visit_expr(&bin.right);
            }
            ast::Expr::Unary(unary) => self.visit_expr(&unary.arg),
            ast::Expr::Update(update) => self.visit_expr(&update.arg),
            ast::Expr::Seq(seq) => {
                for expr in &seq.exprs {
                    self.visit_expr(expr);
                }
            }
            ast::Expr::Assign(assign) => {
                self.visit_expr(&assign.right);
                if let ast::AssignTarget::Simple(simple) = &assign.left {
                    if let ast::SimpleAssignTarget::Member(member) = simple {
                        if is_module_exports_member(&ast::Expr::Member(member.clone()))
                            || is_exports_member(&ast::Expr::Member(member.clone()))
                        {
                            self.found = true;
                            return;
                        }
                        self.visit_expr(&member.obj);
                    }
                }
            }
            ast::Expr::Cond(cond) => {
                self.visit_expr(&cond.test);
                self.visit_expr(&cond.cons);
                self.visit_expr(&cond.alt);
            }
            ast::Expr::Array(arr) => {
                for elem in &arr.elems {
                    if let Some(elem) = elem {
                        self.visit_expr(&elem.expr);
                    }
                }
            }
            ast::Expr::Object(obj) => {
                for prop in &obj.props {
                    match prop {
                        ast::PropOrSpread::Prop(prop) => match prop.as_ref() {
                            ast::Prop::KeyValue(kv) => {
                                self.visit_expr(&kv.value);
                            }
                            ast::Prop::Assign(assign) => {
                                self.visit_expr(&assign.value);
                            }
                            ast::Prop::Getter(getter) => {
                                if let Some(body) = &getter.body {
                                    for stmt in &body.stmts {
                                        self.visit_stmt(stmt);
                                    }
                                }
                            }
                            ast::Prop::Setter(setter) => {
                                if let Some(body) = &setter.body {
                                    for stmt in &body.stmts {
                                        self.visit_stmt(stmt);
                                    }
                                }
                            }
                            ast::Prop::Method(method) => {
                                if let Some(body) = &method.function.body {
                                    for stmt in &body.stmts {
                                        self.visit_stmt(stmt);
                                    }
                                }
                            }
                            _ => {}
                        },
                        ast::PropOrSpread::Spread(spread) => {
                            self.visit_expr(&spread.expr);
                        }
                    }
                }
            }
            ast::Expr::Fn(fn_expr) => {
                if let Some(body) = &fn_expr.function.body {
                    for stmt in &body.stmts {
                        self.visit_stmt(stmt);
                    }
                }
            }
            ast::Expr::Arrow(arrow) => match arrow.body.as_ref() {
                ast::BlockStmtOrExpr::BlockStmt(block) => {
                    for stmt in &block.stmts {
                        self.visit_stmt(stmt);
                    }
                }
                ast::BlockStmtOrExpr::Expr(expr) => self.visit_expr(expr),
            },
            ast::Expr::Paren(paren) => self.visit_expr(&paren.expr),
            ast::Expr::Tpl(tpl) => {
                for expr in &tpl.exprs {
                    self.visit_expr(expr);
                }
            }
            ast::Expr::TaggedTpl(tagged) => {
                self.visit_expr(&tagged.tag);
                for expr in &tagged.tpl.exprs {
                    self.visit_expr(expr);
                }
            }
            ast::Expr::New(new_expr) => {
                self.visit_expr(&new_expr.callee);
                if let Some(args) = &new_expr.args {
                    for arg in args {
                        self.visit_expr(&arg.expr);
                    }
                }
            }
            ast::Expr::Await(await_expr) => self.visit_expr(&await_expr.arg),
            ast::Expr::Yield(yield_expr) => {
                if let Some(arg) = &yield_expr.arg {
                    self.visit_expr(arg);
                }
            }
            ast::Expr::OptChain(opt_chain) => {
                match opt_chain.base.as_ref() {
                    ast::OptChainBase::Member(member) => {
                        self.visit_expr(&member.obj);
                    }
                    ast::OptChainBase::Call(call) => {
                        for arg in &call.args {
                            self.visit_expr(&arg.expr);
                        }
                        self.visit_expr(&call.callee);
                    }
                }
            }
            _ => {}
        }
    }
}

/// 将 CJS 模块转换为 ESM 风格 AST
pub fn transform(module: &ast::Module) -> ast::Module {
    let mut ctx = TransformCtx {
        require_map: HashMap::new(),
        next_req_id: 0,
        exports: Vec::new(),
        export_names: Vec::new(),
        has_default_export: false,
    };

    // 第一遍：收集所有 require() 调用
    for item in &module.body {
        ctx.collect_requires_module_item(item);
    }

    // 第二遍：转换 body
    let mut new_body: Vec<ast::ModuleItem> = Vec::new();

    // 先添加 import 声明（使用默认导入）
    for (specifier, local_name) in &ctx.require_map {
        let import_decl = create_import_default_decl(specifier, local_name);
        new_body.push(ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(
            import_decl,
        )));
    }

    // 再转换原始 body
    for item in &module.body {
        match item {
            ast::ModuleItem::Stmt(stmt) => {
                let transformed = ctx.transform_stmt(stmt);
                match transformed {
                    TransformedStmt::Stmt(stmt) => {
                        new_body.push(ast::ModuleItem::Stmt(stmt));
                    }
                    TransformedStmt::ExportDecl(decl) => {
                        new_body.push(ast::ModuleItem::ModuleDecl(
                            ast::ModuleDecl::ExportDecl(ast::ExportDecl {
                                span: DUMMY_SP,
                                decl,
                            }),
                        ));
                    }
                    TransformedStmt::ExportDefaultExpr(expr) => {
                        new_body.push(ast::ModuleItem::ModuleDecl(
                            ast::ModuleDecl::ExportDefaultExpr(ast::ExportDefaultExpr {
                                span: DUMMY_SP,
                                expr,
                            }),
                        ));
                    }
                    TransformedStmt::Multiple(items) => {
                        for item in items {
                            new_body.push(item);
                        }
                    }
                    TransformedStmt::Skip => {}
                }
            }
            ast::ModuleItem::ModuleDecl(_) => {
                // CJS 模块不应该有 ModuleDecl，保留原样
                new_body.push(item.clone());
            }
        }
    }

    // 如果有命名导出但没有显式默认导出，生成合成默认导出
    if !ctx.export_names.is_empty() && !ctx.has_default_export {
        let default_export_expr = create_synthetic_default_export(&ctx.export_names);
        new_body.push(ast::ModuleItem::ModuleDecl(
            ast::ModuleDecl::ExportDefaultExpr(ast::ExportDefaultExpr {
                span: DUMMY_SP,
                expr: Box::new(default_export_expr),
            }),
        ));
    }

    ast::Module {
        span: module.span,
        body: new_body,
        shebang: module.shebang.clone(),
    }
}

struct TransformCtx {
    /// specifier → local variable name (e.g. "./foo" → "__cjs_req_0")
    require_map: HashMap<String, String>,
    next_req_id: u32,
    /// 收集到的 exports（用于生成 export 声明）
    exports: Vec<(String, ast::Expr)>,
    /// 命名导出的名称列表（用于生成合成默认导出）
    export_names: Vec<String>,
    /// 是否有显式的默认导出（module.exports = obj）
    has_default_export: bool,
}

impl TransformCtx {
    fn collect_requires_module_item(&mut self, item: &ast::ModuleItem) {
        match item {
            ast::ModuleItem::Stmt(stmt) => self.collect_requires_stmt(stmt),
            ast::ModuleItem::ModuleDecl(decl) => self.collect_requires_module_decl(decl),
        }
    }

    fn collect_requires_module_decl(&mut self, decl: &ast::ModuleDecl) {
        match decl {
            ast::ModuleDecl::ExportDecl(export_decl) => {
                if let ast::Decl::Var(var_decl) = &export_decl.decl {
                    for decl in &var_decl.decls {
                        if let Some(init) = &decl.init {
                            self.collect_requires_expr(init);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn collect_requires_stmt(&mut self, stmt: &ast::Stmt) {
        match stmt {
            ast::Stmt::Expr(expr_stmt) => self.collect_requires_expr(&expr_stmt.expr),
            ast::Stmt::Decl(decl) => self.collect_requires_decl(decl),
            ast::Stmt::Block(block) => {
                for stmt in &block.stmts {
                    self.collect_requires_stmt(stmt);
                }
            }
            ast::Stmt::If(if_stmt) => {
                self.collect_requires_expr(&if_stmt.test);
                self.collect_requires_stmt(&if_stmt.cons);
                if let Some(alt) = &if_stmt.alt {
                    self.collect_requires_stmt(alt);
                }
            }
            ast::Stmt::While(while_stmt) => {
                self.collect_requires_expr(&while_stmt.test);
                self.collect_requires_stmt(&while_stmt.body);
            }
            ast::Stmt::DoWhile(do_while) => {
                self.collect_requires_stmt(&do_while.body);
                self.collect_requires_expr(&do_while.test);
            }
            ast::Stmt::For(for_stmt) => {
                if let Some(init) = &for_stmt.init {
                    match init {
                        ast::VarDeclOrExpr::VarDecl(var_decl) => {
                            self.collect_requires_var_decl(var_decl)
                        }
                        ast::VarDeclOrExpr::Expr(expr) => self.collect_requires_expr(expr),
                    }
                }
                if let Some(test) = &for_stmt.test {
                    self.collect_requires_expr(test);
                }
                if let Some(update) = &for_stmt.update {
                    self.collect_requires_expr(update);
                }
                self.collect_requires_stmt(&for_stmt.body);
            }
            ast::Stmt::ForIn(for_in) => {
                self.collect_requires_expr(&for_in.right);
                self.collect_requires_stmt(&for_in.body);
            }
            ast::Stmt::ForOf(for_of) => {
                self.collect_requires_expr(&for_of.right);
                self.collect_requires_stmt(&for_of.body);
            }
            ast::Stmt::Switch(switch) => {
                self.collect_requires_expr(&switch.discriminant);
                for case in &switch.cases {
                    if let Some(test) = &case.test {
                        self.collect_requires_expr(test);
                    }
                    for stmt in &case.cons {
                        self.collect_requires_stmt(stmt);
                    }
                }
            }
            ast::Stmt::Try(try_stmt) => {
                for stmt in &try_stmt.block.stmts {
                    self.collect_requires_stmt(stmt);
                }
                if let Some(handler) = &try_stmt.handler {
                    for stmt in &handler.body.stmts {
                        self.collect_requires_stmt(stmt);
                    }
                }
                if let Some(finalizer) = &try_stmt.finalizer {
                    for stmt in &finalizer.stmts {
                        self.collect_requires_stmt(stmt);
                    }
                }
            }
            ast::Stmt::Labeled(labeled) => self.collect_requires_stmt(&labeled.body),
            ast::Stmt::Return(ret) => {
                if let Some(arg) = &ret.arg {
                    self.collect_requires_expr(arg);
                }
            }
            ast::Stmt::Throw(throw) => self.collect_requires_expr(&throw.arg),
            ast::Stmt::With(with) => {
                self.collect_requires_expr(&with.obj);
                self.collect_requires_stmt(&with.body);
            }
            _ => {}
        }
    }

    fn collect_requires_decl(&mut self, decl: &ast::Decl) {
        match decl {
            ast::Decl::Var(var_decl) => self.collect_requires_var_decl(var_decl),
            ast::Decl::Fn(fn_decl) => {
                if let Some(body) = &fn_decl.function.body {
                    for stmt in &body.stmts {
                        self.collect_requires_stmt(stmt);
                    }
                }
            }
            ast::Decl::Class(class_decl) => {
                if let Some(super_class) = &class_decl.class.super_class {
                    self.collect_requires_expr(super_class);
                }
                for member in &class_decl.class.body {
                    match member {
                        ast::ClassMember::Method(method) => {
                            if let Some(body) = &method.function.body {
                                for stmt in &body.stmts {
                                    self.collect_requires_stmt(stmt);
                                }
                            }
                        }
                        ast::ClassMember::PrivateMethod(method) => {
                            if let Some(body) = &method.function.body {
                                for stmt in &body.stmts {
                                    self.collect_requires_stmt(stmt);
                                }
                            }
                        }
                        ast::ClassMember::Constructor(ctor) => {
                            if let Some(body) = &ctor.body {
                                for stmt in &body.stmts {
                                    self.collect_requires_stmt(stmt);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn collect_requires_var_decl(&mut self, var_decl: &ast::VarDecl) {
        for decl in &var_decl.decls {
            if let Some(init) = &decl.init {
                self.collect_requires_expr(init);
            }
        }
    }

    fn collect_requires_expr(&mut self, expr: &ast::Expr) {
        match expr {
            ast::Expr::Call(call) => {
                if let Some(specifier) = extract_require_specifier(call) {
                    if !self.require_map.contains_key(&specifier) {
                        let local_name = format!("__cjs_req_{}", self.next_req_id);
                        self.next_req_id += 1;
                        self.require_map.insert(specifier, local_name);
                    }
                }
                for arg in &call.args {
                    self.collect_requires_expr(&arg.expr);
                }
                if let ast::Callee::Expr(callee) = &call.callee {
                    self.collect_requires_expr(callee);
                }
            }
            ast::Expr::Member(member) => {
                self.collect_requires_expr(&member.obj);
                if let ast::MemberProp::Computed(computed) = &member.prop {
                    self.collect_requires_expr(&computed.expr);
                }
            }
            ast::Expr::Bin(bin) => {
                self.collect_requires_expr(&bin.left);
                self.collect_requires_expr(&bin.right);
            }
            ast::Expr::Unary(unary) => self.collect_requires_expr(&unary.arg),
            ast::Expr::Update(update) => self.collect_requires_expr(&update.arg),
            ast::Expr::Seq(seq) => {
                for expr in &seq.exprs {
                    self.collect_requires_expr(expr);
                }
            }
            ast::Expr::Assign(assign) => {
                self.collect_requires_expr(&assign.right);
                if let ast::AssignTarget::Simple(simple) = &assign.left {
                    if let ast::SimpleAssignTarget::Member(member) = simple {
                        self.collect_requires_expr(&member.obj);
                    }
                }
            }
            ast::Expr::Cond(cond) => {
                self.collect_requires_expr(&cond.test);
                self.collect_requires_expr(&cond.cons);
                self.collect_requires_expr(&cond.alt);
            }
            ast::Expr::Array(arr) => {
                for elem in &arr.elems {
                    if let Some(elem) = elem {
                        self.collect_requires_expr(&elem.expr);
                    }
                }
            }
            ast::Expr::Object(obj) => {
                for prop in &obj.props {
                    match prop {
                        ast::PropOrSpread::Prop(prop) => match prop.as_ref() {
                            ast::Prop::KeyValue(kv) => {
                                self.collect_requires_expr(&kv.value);
                            }
                            ast::Prop::Assign(assign) => {
                                self.collect_requires_expr(&assign.value);
                            }
                            ast::Prop::Getter(getter) => {
                                if let Some(body) = &getter.body {
                                    for stmt in &body.stmts {
                                        self.collect_requires_stmt(stmt);
                                    }
                                }
                            }
                            ast::Prop::Setter(setter) => {
                                if let Some(body) = &setter.body {
                                    for stmt in &body.stmts {
                                        self.collect_requires_stmt(stmt);
                                    }
                                }
                            }
                            ast::Prop::Method(method) => {
                                if let Some(body) = &method.function.body {
                                    for stmt in &body.stmts {
                                        self.collect_requires_stmt(stmt);
                                    }
                                }
                            }
                            _ => {}
                        },
                        ast::PropOrSpread::Spread(spread) => {
                            self.collect_requires_expr(&spread.expr);
                        }
                    }
                }
            }
            ast::Expr::Fn(fn_expr) => {
                if let Some(body) = &fn_expr.function.body {
                    for stmt in &body.stmts {
                        self.collect_requires_stmt(stmt);
                    }
                }
            }
            ast::Expr::Arrow(arrow) => match arrow.body.as_ref() {
                ast::BlockStmtOrExpr::BlockStmt(block) => {
                    for stmt in &block.stmts {
                        self.collect_requires_stmt(stmt);
                    }
                }
                ast::BlockStmtOrExpr::Expr(expr) => self.collect_requires_expr(expr),
            },
            ast::Expr::Paren(paren) => self.collect_requires_expr(&paren.expr),
            ast::Expr::Tpl(tpl) => {
                for expr in &tpl.exprs {
                    self.collect_requires_expr(expr);
                }
            }
            ast::Expr::TaggedTpl(tagged) => {
                self.collect_requires_expr(&tagged.tag);
                for expr in &tagged.tpl.exprs {
                    self.collect_requires_expr(expr);
                }
            }
            ast::Expr::New(new_expr) => {
                self.collect_requires_expr(&new_expr.callee);
                if let Some(args) = &new_expr.args {
                    for arg in args {
                        self.collect_requires_expr(&arg.expr);
                    }
                }
            }
            ast::Expr::Await(await_expr) => self.collect_requires_expr(&await_expr.arg),
            ast::Expr::Yield(yield_expr) => {
                if let Some(arg) = &yield_expr.arg {
                    self.collect_requires_expr(arg);
                }
            }
            ast::Expr::OptChain(opt_chain) => {
                match opt_chain.base.as_ref() {
                    ast::OptChainBase::Member(member) => {
                        self.collect_requires_expr(&member.obj);
                    }
                    ast::OptChainBase::Call(call) => {
                        for arg in &call.args {
                            self.collect_requires_expr(&arg.expr);
                        }
                        self.collect_requires_expr(&call.callee);
                    }
                }
            }
            _ => {}
        }
    }

    fn transform_stmt(&mut self, stmt: &ast::Stmt) -> TransformedStmt {
        match stmt {
            ast::Stmt::Expr(expr_stmt) => {
                if let Some(transformed) = self.try_transform_expr_stmt(&expr_stmt.expr) {
                    transformed
                } else {
                    TransformedStmt::Stmt(ast::Stmt::Expr(ast::ExprStmt {
                        span: expr_stmt.span,
                        expr: Box::new(self.transform_expr(&expr_stmt.expr)),
                    }))
                }
            }
            ast::Stmt::Decl(decl) => self.transform_decl(decl),
            ast::Stmt::Block(block) => {
                let mut new_stmts = Vec::new();
                for stmt in &block.stmts {
                    match self.transform_stmt(stmt) {
                        TransformedStmt::Stmt(s) => new_stmts.push(s),
                        TransformedStmt::ExportDecl(decl) => {
                            return TransformedStmt::Multiple(vec![
                                ast::ModuleItem::Stmt(ast::Stmt::Block(ast::BlockStmt {
                                    span: block.span,
                                    ctxt: SyntaxContext::default(),
                                    stmts: new_stmts,
                                })),
                                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(
                                    ast::ExportDecl {
                                        span: DUMMY_SP,
                                        decl,
                                    },
                                )),
                            ]);
                        }
                        TransformedStmt::ExportDefaultExpr(expr) => {
                            return TransformedStmt::Multiple(vec![
                                ast::ModuleItem::Stmt(ast::Stmt::Block(ast::BlockStmt {
                                    span: block.span,
                                    ctxt: SyntaxContext::default(),
                                    stmts: new_stmts,
                                })),
                                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDefaultExpr(
                                    ast::ExportDefaultExpr {
                                        span: DUMMY_SP,
                                        expr,
                                    },
                                )),
                            ]);
                        }
                        TransformedStmt::Multiple(items) => {
                            return TransformedStmt::Multiple(vec![
                                ast::ModuleItem::Stmt(ast::Stmt::Block(ast::BlockStmt {
                                    span: block.span,
                                    ctxt: SyntaxContext::default(),
                                    stmts: new_stmts,
                                })),
                            ]
                            .into_iter()
                            .chain(items.into_iter())
                            .collect());
                        }
                        TransformedStmt::Skip => {}
                    }
                }
                TransformedStmt::Stmt(ast::Stmt::Block(ast::BlockStmt {
                    span: block.span,
                    ctxt: SyntaxContext::default(),
                    stmts: new_stmts,
                }))
            }
            ast::Stmt::If(if_stmt) => {
                let test = self.transform_expr(&if_stmt.test);
                let cons = match self.transform_stmt(&if_stmt.cons) {
                    TransformedStmt::Stmt(s) => s,
                    other => {
                        // 简化处理：if 体中有 export 时，将 if 展开为块
                        return TransformedStmt::Stmt(ast::Stmt::If(ast::IfStmt {
                            span: if_stmt.span,
                            test: Box::new(test),
                            cons: Box::new(stmt_from_transformed(other)),
                            alt: if_stmt.alt.clone(),
                        }));
                    }
                };
                let alt = if_stmt.alt.as_ref().map(|a| match self.transform_stmt(a) {
                    TransformedStmt::Stmt(s) => Box::new(s),
                    other => Box::new(stmt_from_transformed(other)),
                });
                TransformedStmt::Stmt(ast::Stmt::If(ast::IfStmt {
                    span: if_stmt.span,
                    test: Box::new(test),
                    cons: Box::new(cons),
                    alt,
                }))
            }
            ast::Stmt::While(while_stmt) => TransformedStmt::Stmt(ast::Stmt::While(ast::WhileStmt {
                span: while_stmt.span,
                test: Box::new(self.transform_expr(&while_stmt.test)),
                body: Box::new(match self.transform_stmt(&while_stmt.body) {
                    TransformedStmt::Stmt(s) => s,
                    other => stmt_from_transformed(other),
                }),
            })),
            ast::Stmt::DoWhile(do_while) => {
                TransformedStmt::Stmt(ast::Stmt::DoWhile(ast::DoWhileStmt {
                    span: do_while.span,
                    test: Box::new(self.transform_expr(&do_while.test)),
                    body: Box::new(match self.transform_stmt(&do_while.body) {
                        TransformedStmt::Stmt(s) => s,
                        other => stmt_from_transformed(other),
                    }),
                }))
            }
            ast::Stmt::For(for_stmt) => {
                let init = for_stmt.init.as_ref().map(|init| match init {
                    ast::VarDeclOrExpr::VarDecl(var_decl) => {
                        ast::VarDeclOrExpr::VarDecl(Box::new(self.transform_var_decl(var_decl)))
                    }
                    ast::VarDeclOrExpr::Expr(expr) => {
                        ast::VarDeclOrExpr::Expr(Box::new(self.transform_expr(expr)))
                    }
                });
                TransformedStmt::Stmt(ast::Stmt::For(ast::ForStmt {
                    span: for_stmt.span,
                    init,
                    test: for_stmt.test.as_ref().map(|e| Box::new(self.transform_expr(e))),
                    update: for_stmt
                        .update
                        .as_ref()
                        .map(|e| Box::new(self.transform_expr(e))),
                    body: Box::new(match self.transform_stmt(&for_stmt.body) {
                        TransformedStmt::Stmt(s) => s,
                        other => stmt_from_transformed(other),
                    }),
                }))
            }
            ast::Stmt::ForIn(for_in) => TransformedStmt::Stmt(ast::Stmt::ForIn(ast::ForInStmt {
                span: for_in.span,
                left: for_in.left.clone(),
                right: Box::new(self.transform_expr(&for_in.right)),
                body: Box::new(match self.transform_stmt(&for_in.body) {
                    TransformedStmt::Stmt(s) => s,
                    other => stmt_from_transformed(other),
                }),
            })),
            ast::Stmt::ForOf(for_of) => TransformedStmt::Stmt(ast::Stmt::ForOf(ast::ForOfStmt {
                span: for_of.span,
                is_await: for_of.is_await,
                left: for_of.left.clone(),
                right: Box::new(self.transform_expr(&for_of.right)),
                body: Box::new(match self.transform_stmt(&for_of.body) {
                    TransformedStmt::Stmt(s) => s,
                    other => stmt_from_transformed(other),
                }),
            })),
            ast::Stmt::Switch(switch) => {
                let mut new_cases = Vec::new();
                for case in &switch.cases {
                    let mut new_cons = Vec::new();
                    for stmt in &case.cons {
                        match self.transform_stmt(stmt) {
                            TransformedStmt::Stmt(s) => new_cons.push(s),
                            other => {
                                // 简化：switch case 中有 export 时，直接保留原样
                                new_cons.push(stmt.clone());
                            }
                        }
                    }
                    new_cases.push(ast::SwitchCase {
                        span: case.span,
                        test: case.test.clone(),
                        cons: new_cons,
                    });
                }
                TransformedStmt::Stmt(ast::Stmt::Switch(ast::SwitchStmt {
                    span: switch.span,
                    discriminant: Box::new(self.transform_expr(&switch.discriminant)),
                    cases: new_cases,
                }))
            }
            ast::Stmt::Try(try_stmt) => {
                let mut new_block_stmts = Vec::new();
                for stmt in &try_stmt.block.stmts {
                    match self.transform_stmt(stmt) {
                        TransformedStmt::Stmt(s) => new_block_stmts.push(s),
                        other => {
                            new_block_stmts.push(stmt.clone());
                        }
                    }
                }
                TransformedStmt::Stmt(ast::Stmt::Try(Box::new(ast::TryStmt {
                    span: try_stmt.span,
                    block: ast::BlockStmt {
                        span: try_stmt.block.span,
                        ctxt: SyntaxContext::default(),
                        stmts: new_block_stmts,
                    },
                    handler: try_stmt.handler.clone(),
                    finalizer: try_stmt.finalizer.clone(),
                })))
            }
            ast::Stmt::Labeled(labeled) => {
                TransformedStmt::Stmt(ast::Stmt::Labeled(ast::LabeledStmt {
                    span: labeled.span,
                    label: labeled.label.clone(),
                    body: Box::new(match self.transform_stmt(&labeled.body) {
                        TransformedStmt::Stmt(s) => s,
                        other => stmt_from_transformed(other),
                    }),
                }))
            }
            ast::Stmt::Return(ret) => TransformedStmt::Stmt(ast::Stmt::Return(ast::ReturnStmt {
                span: ret.span,
                arg: ret.arg.as_ref().map(|e| Box::new(self.transform_expr(e))),
            })),
            ast::Stmt::Throw(throw) => TransformedStmt::Stmt(ast::Stmt::Throw(ast::ThrowStmt {
                span: throw.span,
                arg: Box::new(self.transform_expr(&throw.arg)),
            })),
            ast::Stmt::With(with) => TransformedStmt::Stmt(ast::Stmt::With(ast::WithStmt {
                span: with.span,
                obj: Box::new(self.transform_expr(&with.obj)),
                body: Box::new(match self.transform_stmt(&with.body) {
                    TransformedStmt::Stmt(s) => s,
                    other => stmt_from_transformed(other),
                }),
            })),
            ast::Stmt::Empty(_) => TransformedStmt::Stmt(stmt.clone()),
            ast::Stmt::Debugger(_) => TransformedStmt::Stmt(stmt.clone()),
            ast::Stmt::Break(_) => TransformedStmt::Stmt(stmt.clone()),
            ast::Stmt::Continue(_) => TransformedStmt::Stmt(stmt.clone()),
        }
    }

    fn try_transform_expr_stmt(&mut self, expr: &ast::Expr) -> Option<TransformedStmt> {
        // 处理 module.exports.x = value
        if let ast::Expr::Assign(assign) = expr {
            if assign.op == ast::AssignOp::Assign {
                if let ast::AssignTarget::Simple(simple) = &assign.left {
                    if let ast::SimpleAssignTarget::Member(member) = simple {
                        if is_module_exports_member(&member.obj) {
                            // module.exports.x = value → export let x = value
                            let prop_name = match &member.prop {
                                ast::MemberProp::Ident(ident) => ident.sym.to_string(),
                                ast::MemberProp::Computed(computed) => {
                                    if let ast::Expr::Lit(ast::Lit::Str(s)) = computed.expr.as_ref()
                                    {
                                        s.value.to_string_lossy().into_owned()
                                    } else {
                                        return None;
                                    }
                                }
                                _ => return None,
                            };
                            let value = self.transform_expr(&assign.right);
                            self.export_names.push(prop_name.clone());
                            let decl = create_let_decl(&prop_name, value);
                            return Some(TransformedStmt::ExportDecl(decl));
                        }
                        if is_exports_ident(&member.obj) {
                            // exports.x = value → export let x = value
                            let prop_name = match &member.prop {
                                ast::MemberProp::Ident(ident) => ident.sym.to_string(),
                                ast::MemberProp::Computed(computed) => {
                                    if let ast::Expr::Lit(ast::Lit::Str(s)) = computed.expr.as_ref()
                                    {
                                        s.value.to_string_lossy().into_owned()
                                    } else {
                                        return None;
                                    }
                                }
                                _ => return None,
                            };
                            let value = self.transform_expr(&assign.right);
                            self.export_names.push(prop_name.clone());
                            let decl = create_let_decl(&prop_name, value);
                            return Some(TransformedStmt::ExportDecl(decl));
                        }
                    }
                }
            }
        }

        // 处理 module.exports = value
        if let ast::Expr::Assign(assign) = expr {
            if assign.op == ast::AssignOp::Assign {
                if let ast::AssignTarget::Simple(simple) = &assign.left {
                    if let ast::SimpleAssignTarget::Member(member) = simple {
                        if is_module_exports_member_no_prop(&member.obj, &member.prop) {
                            let value = self.transform_expr(&assign.right);
                            self.has_default_export = true;
                            return Some(TransformedStmt::ExportDefaultExpr(Box::new(value)));
                        }
                    }
                }
            }
        }

        None
    }

    fn transform_decl(&mut self, decl: &ast::Decl) -> TransformedStmt {
        match decl {
            ast::Decl::Var(var_decl) => {
                TransformedStmt::Stmt(ast::Stmt::Decl(ast::Decl::Var(Box::new(
                    self.transform_var_decl(var_decl),
                ))))
            }
            ast::Decl::Fn(fn_decl) => TransformedStmt::Stmt(ast::Stmt::Decl(ast::Decl::Fn(
                ast::FnDecl {
                    ident: fn_decl.ident.clone(),
                    declare: fn_decl.declare,
                    function: fn_decl.function.clone(),
                },
            ))),
            ast::Decl::Class(class_decl) => TransformedStmt::Stmt(ast::Stmt::Decl(
                ast::Decl::Class(ast::ClassDecl {
                    ident: class_decl.ident.clone(),
                    declare: class_decl.declare,
                    class: class_decl.class.clone(),
                }),
            )),
            _ => TransformedStmt::Stmt(ast::Stmt::Decl(decl.clone())),
        }
    }

    fn transform_var_decl(&mut self, var_decl: &ast::VarDecl) -> ast::VarDecl {
        let mut new_decls = Vec::new();
        for decl in &var_decl.decls {
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
                    ast::Callee::Super(_) => call.callee.clone(),
                    ast::Callee::Import(_) => call.callee.clone(),
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
            ast::Expr::Member(member) => {
                let new_obj = Box::new(self.transform_expr(&member.obj));
                let new_prop = match &member.prop {
                    ast::MemberProp::Ident(ident) => ast::MemberProp::Ident(ident.clone()),
                    ast::MemberProp::PrivateName(name) => ast::MemberProp::PrivateName(name.clone()),
                    ast::MemberProp::Computed(computed) => ast::MemberProp::Computed(
                        ast::ComputedPropName {
                            span: computed.span,
                            expr: Box::new(self.transform_expr(&computed.expr)),
                        },
                    ),
                };
                ast::Expr::Member(ast::MemberExpr {
                    span: member.span,
                    obj: new_obj,
                    prop: new_prop,
                })
            }
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
            ast::Expr::Seq(seq) => ast::Expr::Seq(ast::SeqExpr {
                span: seq.span,
                exprs: seq
                    .exprs
                    .iter()
                    .map(|e| Box::new(self.transform_expr(e)))
                    .collect(),
            }),
            ast::Expr::Assign(assign) => {
                let new_right = Box::new(self.transform_expr(&assign.right));
                let new_left = match &assign.left {
                    ast::AssignTarget::Simple(simple) => match simple {
                        ast::SimpleAssignTarget::Ident(ident) => {
                            ast::AssignTarget::Simple(ast::SimpleAssignTarget::Ident(ident.clone()))
                        }
                        ast::SimpleAssignTarget::Member(member) => {
                            ast::AssignTarget::Simple(ast::SimpleAssignTarget::Member(
                                ast::MemberExpr {
                                    span: member.span,
                                    obj: Box::new(self.transform_expr(&member.obj)),
                                    prop: member.prop.clone(),
                                },
                            ))
                        }
                        ast::SimpleAssignTarget::Paren(paren) => {
                            ast::AssignTarget::Simple(ast::SimpleAssignTarget::Paren(
                                ast::ParenExpr {
                                    span: paren.span,
                                    expr: Box::new(self.transform_expr(&paren.expr)),
                                },
                            ))
                        }
                        ast::SimpleAssignTarget::SuperProp(prop) => {
                            ast::AssignTarget::Simple(ast::SimpleAssignTarget::SuperProp(
                                prop.clone(),
                            ))
                        }
                        ast::SimpleAssignTarget::TsAs(_) | ast::SimpleAssignTarget::TsSatisfies(_) | ast::SimpleAssignTarget::TsNonNull(_) | ast::SimpleAssignTarget::TsTypeAssertion(_) | ast::SimpleAssignTarget::TsInstantiation(_) => {
                            // TypeScript-only, keep as-is
                            ast::AssignTarget::Simple(simple.clone())
                        }
                        ast::SimpleAssignTarget::OptChain(_) | ast::SimpleAssignTarget::Invalid(_) => {
                            ast::AssignTarget::Simple(simple.clone())
                        }
                    },
                    ast::AssignTarget::Pat(pat) => ast::AssignTarget::Pat(pat.clone()),
                };
                ast::Expr::Assign(ast::AssignExpr {
                    span: assign.span,
                    op: assign.op,
                    left: new_left,
                    right: new_right,
                })
            }
            ast::Expr::Cond(cond) => ast::Expr::Cond(ast::CondExpr {
                span: cond.span,
                test: Box::new(self.transform_expr(&cond.test)),
                cons: Box::new(self.transform_expr(&cond.cons)),
                alt: Box::new(self.transform_expr(&cond.alt)),
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
                                ast::Prop::Assign(assign) => {
                                    ast::Prop::Assign(ast::AssignProp {
                                        span: assign.span,
                                        key: assign.key.clone(),
                                        value: Box::new(self.transform_expr(&assign.value)),
                                    })
                                }
                                ast::Prop::Getter(getter) => ast::Prop::Getter(ast::GetterProp {
                                    span: getter.span,
                                    key: getter.key.clone(),
                                    type_ann: getter.type_ann.clone(),
                                    body: getter.body.clone(),
                                }),
                                ast::Prop::Setter(setter) => ast::Prop::Setter(ast::SetterProp {
                                    span: setter.span,
                                    key: setter.key.clone(),
                                    this_param: setter.this_param.clone(),
                                    param: setter.param.clone(),
                                    body: setter.body.clone(),
                                }),
                                ast::Prop::Method(method) => ast::Prop::Method(ast::MethodProp {
                                    key: method.key.clone(),
                                    function: method.function.clone(),
                                }),
                                ast::Prop::Shorthand(shorthand) => {
                                    ast::Prop::Shorthand(shorthand.clone())
                                }
                            }))
                        }
                        ast::PropOrSpread::Spread(spread) => {
                            ast::PropOrSpread::Spread(ast::SpreadElement {
                                dot3_token: spread.dot3_token,
                                expr: Box::new(self.transform_expr(&spread.expr)),
                            })
                        }
                    })
                    .collect(),
            }),
            ast::Expr::Fn(fn_expr) => ast::Expr::Fn(ast::FnExpr {
                ident: fn_expr.ident.clone(),
                function: fn_expr.function.clone(),
            }),
            ast::Expr::Arrow(arrow) => ast::Expr::Arrow(ast::ArrowExpr {
                span: arrow.span,
                ctxt: SyntaxContext::default(),
                params: arrow.params.clone(),
                body: match arrow.body.as_ref() {
                    ast::BlockStmtOrExpr::BlockStmt(block) => {
                        Box::new(ast::BlockStmtOrExpr::BlockStmt(ast::BlockStmt {
                            span: block.span,
                            ctxt: SyntaxContext::default(),
                            stmts: block
                                .stmts
                                .iter()
                                .map(|stmt| match self.transform_stmt(stmt) {
                                    TransformedStmt::Stmt(s) => s,
                                    other => stmt_from_transformed(other),
                                })
                                .collect(),
                        }))
                    }
                    ast::BlockStmtOrExpr::Expr(expr) => {
                        Box::new(ast::BlockStmtOrExpr::Expr(Box::new(self.transform_expr(expr))))
                    }
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
            ast::Expr::TaggedTpl(tagged) => ast::Expr::TaggedTpl(ast::TaggedTpl {
                span: tagged.span,
                ctxt: SyntaxContext::default(),
                tag: Box::new(self.transform_expr(&tagged.tag)),
                tpl: Box::new(ast::Tpl {
                    span: tagged.tpl.span,
                    exprs: tagged
                        .tpl
                        .exprs
                        .iter()
                        .map(|e| Box::new(self.transform_expr(e)))
                        .collect(),
                    quasis: tagged.tpl.quasis.clone(),
                }),
                type_params: tagged.type_params.clone(),
            }),
            ast::Expr::New(new_expr) => ast::Expr::New(ast::NewExpr {
                span: new_expr.span,
                ctxt: SyntaxContext::default(),
                callee: Box::new(self.transform_expr(&new_expr.callee)),
                args: new_expr.args.as_ref().map(|args| {
                    args.iter()
                        .map(|arg| ast::ExprOrSpread {
                            spread: arg.spread,
                            expr: Box::new(self.transform_expr(&arg.expr)),
                        })
                        .collect()
                }),
                type_args: new_expr.type_args.clone(),
            }),
            ast::Expr::Await(await_expr) => ast::Expr::Await(ast::AwaitExpr {
                span: await_expr.span,
                arg: Box::new(self.transform_expr(&await_expr.arg)),
            }),
            ast::Expr::Yield(yield_expr) => ast::Expr::Yield(ast::YieldExpr {
                span: yield_expr.span,
                arg: yield_expr
                    .arg
                    .as_ref()
                    .map(|e| Box::new(self.transform_expr(e))),
                delegate: yield_expr.delegate,
            }),
            ast::Expr::OptChain(opt_chain) => ast::Expr::OptChain(ast::OptChainExpr {
                span: opt_chain.span,
                optional: opt_chain.optional,
                base: Box::new(match opt_chain.base.as_ref() {
                    ast::OptChainBase::Member(member) => ast::OptChainBase::Member(ast::MemberExpr {
                        span: member.span,
                        obj: Box::new(self.transform_expr(&member.obj)),
                        prop: member.prop.clone(),
                    }),
                    ast::OptChainBase::Call(call) => ast::OptChainBase::Call(ast::OptCall {
                        span: call.span,
                        ctxt: SyntaxContext::default(),
                        callee: Box::new(self.transform_expr(&call.callee)),
                        args: call
                            .args
                            .iter()
                            .map(|arg| ast::ExprOrSpread {
                                spread: arg.spread,
                                expr: Box::new(self.transform_expr(&arg.expr)),
                            })
                            .collect(),
                        type_args: call.type_args.clone(),
                    }),
                }),
            }),
            ast::Expr::This(_) => expr.clone(),
            ast::Expr::Ident(_) => expr.clone(),
            ast::Expr::Lit(_) => expr.clone(),
            ast::Expr::SuperProp(_) => expr.clone(),
            ast::Expr::Class(class_expr) => ast::Expr::Class(ast::ClassExpr {
                ident: class_expr.ident.clone(),
                class: class_expr.class.clone(),
            }),
            ast::Expr::MetaProp(_) => expr.clone(),
            ast::Expr::PrivateName(_) => expr.clone(),
            ast::Expr::Invalid(_) => expr.clone(),
            ast::Expr::TsAs(_) | ast::Expr::TsSatisfies(_) | ast::Expr::TsConstAssertion(_) | ast::Expr::TsInstantiation(_) | ast::Expr::TsNonNull(_) | ast::Expr::TsTypeAssertion(_) => expr.clone(),
            ast::Expr::JSXMember(_) | ast::Expr::JSXNamespacedName(_) | ast::Expr::JSXEmpty(_) | ast::Expr::JSXElement(_) | ast::Expr::JSXFragment(_) => expr.clone(),
        }
    }
}

enum TransformedStmt {
    Stmt(ast::Stmt),
    ExportDecl(ast::Decl),
    ExportDefaultExpr(Box<ast::Expr>),
    Multiple(Vec<ast::ModuleItem>),
    Skip,
}

fn stmt_from_transformed(transformed: TransformedStmt) -> ast::Stmt {
    match transformed {
        TransformedStmt::Stmt(stmt) => stmt,
        TransformedStmt::ExportDecl(decl) => ast::Stmt::Decl(decl),
        TransformedStmt::ExportDefaultExpr(expr) => ast::Stmt::Expr(ast::ExprStmt {
            span: DUMMY_SP,
            expr,
        }),
        TransformedStmt::Multiple(items) => {
            // 将多个 ModuleItem 包装为 IIFE 调用（简化处理）
            let stmts: Vec<ast::Stmt> = items
                .into_iter()
                .filter_map(|item| match item {
                    ast::ModuleItem::Stmt(stmt) => Some(stmt),
                    ast::ModuleItem::ModuleDecl(decl) => match decl {
                        ast::ModuleDecl::ExportDecl(export_decl) => {
                            Some(ast::Stmt::Decl(export_decl.decl))
                        }
                        ast::ModuleDecl::ExportDefaultExpr(default_expr) => {
                            Some(ast::Stmt::Expr(ast::ExprStmt {
                                span: DUMMY_SP,
                                expr: default_expr.expr,
                            }))
                        }
                        _ => None,
                    },
                })
                .collect();
            ast::Stmt::Block(ast::BlockStmt {
                span: DUMMY_SP,
                ctxt: SyntaxContext::default(),
                stmts,
            })
        }
        TransformedStmt::Skip => ast::Stmt::Empty(ast::EmptyStmt { span: DUMMY_SP }),
    }
}

/// 判断是否是 require() 调用
fn is_require_call(call: &ast::CallExpr) -> bool {
    extract_require_specifier(call).is_some()
}

/// 从 require() 调用中提取模块路径
fn extract_require_specifier(call: &ast::CallExpr) -> Option<String> {
    // 检查 callee 是否是标识符 "require"
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

/// 判断是否是 module.exports.xxx 成员表达式
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

/// 判断是否是 module.exports（无后续属性）
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

/// 判断是否是 exports.xxx 成员表达式
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

/// 判断是否是 exports 标识符
fn is_exports_ident(expr: &ast::Expr) -> bool {
    matches!(expr, ast::Expr::Ident(ident) if ident.sym.as_ref() == "exports")
}

/// 创建 `import local_name from 'specifier'` 声明（默认导入）
fn create_import_default_decl(specifier: &str, local_name: &str) -> ast::ImportDecl {
    ast::ImportDecl {
        span: DUMMY_SP,
        phase: ast::ImportPhase::Evaluation,
        specifiers: vec![ast::ImportSpecifier::Default(ast::ImportDefaultSpecifier {
            span: DUMMY_SP,
            local: ast::Ident::new(
                local_name.into(),
                DUMMY_SP,
                SyntaxContext::default(),
            ),
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

/// 创建合成默认导出对象：{ name1, name2, ... }
fn create_synthetic_default_export(export_names: &[String]) -> ast::Expr {
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
    ast::Expr::Object(ast::ObjectLit {
        span: DUMMY_SP,
        props,
    })
}

/// 创建 `let name = value` 声明
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
    fn transforms_require() {
        let module = parse(r#"const foo = require('./foo'); console.log(foo);"#);
        let transformed = transform(&module);
        let has_default_import = transformed.body.iter().any(|item| {
            if let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import)) = item {
                import.specifiers.iter().any(|s| matches!(s, ast::ImportSpecifier::Default(_)))
            } else {
                false
            }
        });
        assert!(has_default_import, "transformed module should have default import decl");
    }

    #[test]
    fn transforms_module_exports() {
        let module = parse(r#"module.exports.foo = 42;"#);
        let transformed = transform(&module);
        let has_export = transformed.body.iter().any(|item| {
            matches!(
                item,
                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(_))
            )
        });
        assert!(has_export, "transformed module should have export decl");
        // 应该有合成默认导出
        let has_default_export = transformed.body.iter().any(|item| {
            matches!(
                item,
                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDefaultExpr(_))
            )
        });
        assert!(has_default_export, "transformed module should have synthetic default export");
    }

    #[test]
    fn transforms_exports_alias() {
        let module = parse(r#"exports.bar = 42;"#);
        let transformed = transform(&module);
        let has_export = transformed.body.iter().any(|item| {
            matches!(
                item,
                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(_))
            )
        });
        assert!(has_export, "transformed module should have export decl");
    }

    #[test]
    fn transforms_module_exports_default() {
        let module = parse(r#"module.exports = { foo: 1 };"#);
        let transformed = transform(&module);
        let has_default_export = transformed.body.iter().any(|item| {
            matches!(
                item,
                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDefaultExpr(_))
            )
        });
        assert!(
            has_default_export,
            "transformed module should have default export"
        );
    }
}
