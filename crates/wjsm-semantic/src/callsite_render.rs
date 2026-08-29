//! 调用/构造站点 callee 表达式的静态渲染（V8 CallPrinter 同型）。
//!
//! V8 在 kCalledNonCallable / kNotConstructor 拒绝时用 CallPrinter 重放源码
//! AST，渲染失败调用点的 callee 表达式（`o.foo is not a function`）。渲染
//! 结果只依赖 AST 与失败位置，与运行时值无关，因此可在语义层降级时一次
//! 静态求出，经 `Instruction::Call/ConstructCall` 的 `callsite` 字段与
//! 后端反馈槽编号传给宿主拒绝路径。
//!
//! 本模块逐条对齐 CallPrinter 的 print 模式语义（`found_ = true`）：
//! - `Find(node, print=true)`：访问节点；若一个字符都没打印则补
//!   `(intermediate value)`；
//! - `Find(node, print=false)`：不访问节点，直接打印 `(intermediate value)`；
//! - 失败调用点本身不打印 `(...)`，只渲染 callee；子调用打印 `callee(...)`。
//!
//! V8 解析器在建 AST 时做的字面量折叠（`(1 + 2)()` → `3 is not a function`、
//! `(-1)` → `-1`、无插值模板 → 字符串字面量）会改变 CallPrinter 的输入，
//! 此处按同样规则先折叠再渲染。与 `render_destructure_callsite`（解构
//! TypeError 的简化渲染）不同，两者对应 V8 中不同的消息路径与打印入口，
//! 不共享实现。

use swc_core::ecma::ast as swc_ast;

/// CallPrinter 对无法打印内容的节点的占位文本。
const INTERMEDIATE: &str = "(intermediate value)";

/// 渲染失败调用/构造点的 callee 表达式（CallPrinter 顶层 `Find(callee,
/// print=true)`）。`new X()` 的 callee 渲染与调用一致（VisitCallNew 在失败
/// 位置同样只渲染 callee）。callee 为裸 `OptChain` 包装时是链内延续
/// （`o?.foo()` 的失败调用位），对应 V8 链内无包装节点，穿透渲染；
/// 括号包裹的完整链（`(o?.foo)()`）是链边界，按 VisitOptionalChain 占位。
pub(crate) fn render_call_callsite(callee: &swc_ast::Expr) -> Box<str> {
    let mut printer = Printer::default();
    printer.visit_call_callee(callee);
    printer.out.into_boxed_str()
}

/// 渲染 `obj.method(...)` 形态调用点的 callee（intrinsic 原型方法守卫慢
/// 路径持有的是 `MemberExpr` 而非 `Expr`，与 [`render_call_callsite`] 对
/// `Expr::Member` 的渲染完全一致）。
pub(crate) fn render_member_callsite(member: &swc_ast::MemberExpr) -> Box<str> {
    let mut printer = Printer::default();
    printer.visit_member(member, false);
    printer.out.into_boxed_str()
}

/// 解析期字面量折叠结果（V8 parser 的 literal shortcut 语义子集）。
enum FoldedLit {
    Num(f64),
    Bool(bool),
    Null,
    Str(String),
    /// BigInt 字面量：CallPrinter 的 PrintLiteral 对 BigInt 不打印任何内容，
    /// 由 Find 补 `(intermediate value)`。
    BigInt,
}

#[derive(Default)]
struct Printer {
    out: String,
    prints: u32,
}

impl Printer {
    fn print(&mut self, text: &str) {
        self.prints += 1;
        self.out.push_str(text);
    }

    /// CallPrinter::Find 的 print 模式：print=true 访问节点、空产出补占位；
    /// print=false 不访问直接占位。
    fn find_expr(&mut self, expr: &swc_ast::Expr, print: bool) {
        if !print {
            self.print(INTERMEDIATE);
            return;
        }
        let before = self.prints;
        self.visit_expr(expr);
        if self.prints == before {
            self.print(INTERMEDIATE);
        }
    }

    fn visit_expr(&mut self, expr: &swc_ast::Expr) {
        let expr = strip_ts(expr);
        if let Some(lit) = fold_literal(expr) {
            self.print_lit(&lit, true);
            return;
        }
        match expr {
            swc_ast::Expr::Paren(paren) => {
                // V8 无括号节点；括号包裹完整可选链时链顶留有 OptionalChain
                // 包装（VisitOptionalChain 以 print=false 转发 → 占位）。
                let inner = strip_ts(&paren.expr);
                if matches!(inner, swc_ast::Expr::OptChain(_)) {
                    self.print(INTERMEDIATE);
                } else {
                    self.visit_expr(inner);
                }
            }
            swc_ast::Expr::Ident(ident) => self.print(ident.sym.as_ref()),
            swc_ast::Expr::This(_) => self.print("this"),
            swc_ast::Expr::Lit(swc_ast::Lit::Regex(regex)) => {
                let flags = canonical_regexp_flags(regex.flags.as_ref());
                self.print(&format!("/{}/{flags}", regex.exp));
            }
            swc_ast::Expr::Tpl(tpl) => {
                // 有插值的模板：TemplateLiteral 只渲染插值表达式，quasi 不打印。
                for substitution in &tpl.exprs {
                    self.find_expr(substitution, true);
                }
            }
            swc_ast::Expr::Member(member) => self.visit_member(member, false),
            swc_ast::Expr::SuperProp(super_prop) => self.visit_super_prop(super_prop),
            swc_ast::Expr::OptChain(chain) => self.visit_opt_chain(chain),
            swc_ast::Expr::Call(call) => self.visit_sub_call(call),
            swc_ast::Expr::TaggedTpl(tagged) => {
                // tagged template 在 V8 AST 中即 Call，子位置渲染 tag(...)。
                self.find_expr(&tagged.tag, true);
                self.print("(...)");
            }
            swc_ast::Expr::New(_) => {
                // VisitCallNew 子位置：Find(callee, print=false) 占位，实参跳过。
                self.print(INTERMEDIATE);
            }
            swc_ast::Expr::Unary(unary) => {
                let op = unary.op.as_str();
                self.print("(");
                self.print(op);
                if matches!(
                    unary.op,
                    swc_ast::UnaryOp::Delete | swc_ast::UnaryOp::TypeOf | swc_ast::UnaryOp::Void
                ) {
                    self.print(" ");
                }
                self.find_expr(&unary.arg, true);
                self.print(")");
            }
            swc_ast::Expr::Update(update) => {
                self.print("(");
                if update.prefix {
                    self.print(update.op.as_str());
                }
                self.find_expr(&update.arg, true);
                if !update.prefix {
                    self.print(update.op.as_str());
                }
                self.print(")");
            }
            swc_ast::Expr::Bin(bin) => self.visit_bin(bin),
            swc_ast::Expr::Seq(seq) => {
                // 逗号链在 V8 中是 (N)aryOperation，token 文本为 ","。
                self.print("(");
                for (index, item) in seq.exprs.iter().enumerate() {
                    if index > 0 {
                        self.print(" , ");
                    }
                    self.find_expr(item, true);
                }
                self.print(")");
            }
            swc_ast::Expr::Cond(cond) => {
                // VisitConditional 对三个子表达式都以 print=false 转发。
                self.find_expr(&cond.test, false);
                self.find_expr(&cond.cons, false);
                self.find_expr(&cond.alt, false);
            }
            swc_ast::Expr::Assign(assign) => self.visit_assign_target(&assign.left),
            swc_ast::Expr::Arrow(arrow) => match arrow.body.as_ref() {
                swc_ast::BlockStmtOrExpr::BlockStmt(block) => self.visit_fn_body(block.stmts.len()),
                // 表达式体在 V8 中脱糖为单条 return 语句。
                swc_ast::BlockStmtOrExpr::Expr(_) => self.visit_fn_body(1),
            },
            swc_ast::Expr::Fn(fn_expr) => {
                let statements = fn_expr
                    .function
                    .body
                    .as_ref()
                    .map_or(0, |body| body.stmts.len());
                self.visit_fn_body(statements);
            }
            swc_ast::Expr::Class(class_expr) => self.visit_class(&class_expr.class),
            swc_ast::Expr::Object(object) => {
                // ObjectLiteral：打印花括号，属性值一律 print=false 占位。
                self.print("{");
                for _ in &object.props {
                    self.print(INTERMEDIATE);
                }
                self.print("}");
            }
            swc_ast::Expr::Array(array) => self.visit_array(array),
            swc_ast::Expr::Await(await_expr) => {
                // VisitAwait/VisitYield 以 print=false 转发子表达式。
                self.find_expr(&await_expr.arg, false);
            }
            swc_ast::Expr::Yield(yield_expr) => match yield_expr.arg.as_deref() {
                Some(arg) => self.find_expr(arg, false),
                None => self.print(INTERMEDIATE),
            },
            swc_ast::Expr::MetaProp(meta) => {
                if meta.kind == swc_ast::MetaPropKind::NewTarget {
                    // V8 把 new.target 解析为名为 ".new.target" 的 VariableProxy。
                    self.print(".new.target");
                }
            }
            // 其余形态（JSX、Invalid、import.meta 等）不打印任何内容，
            // 由外层 Find 补 `(intermediate value)`。
            _ => {}
        }
    }

    /// 成员访问（VisitProperty）：字符串字面量键为 `.key`（含可选链 `?` 前
    /// 缀），其余键为 `[key]` 形态。
    fn visit_member(&mut self, member: &swc_ast::MemberExpr, optional: bool) {
        self.visit_member_obj(&member.obj);
        match &member.prop {
            swc_ast::MemberProp::Ident(ident) => {
                if optional {
                    self.print("?");
                }
                self.print(".");
                self.print(ident.sym.as_ref());
            }
            swc_ast::MemberProp::PrivateName(name) => {
                // V8 把私有名脱糖为名为 "#name" 的 VariableProxy 计算键。
                if optional {
                    self.print("?.");
                }
                self.print("[");
                self.print(&format!("#{}", name.name));
                self.print("]");
            }
            swc_ast::MemberProp::Computed(computed) => {
                match fold_literal(strip_ts(&computed.expr)) {
                    Some(FoldedLit::Str(text)) => {
                        // internalized string 键与点访问同渲染，原文不加引号。
                        if optional {
                            self.print("?");
                        }
                        self.print(".");
                        self.print(&text);
                    }
                    _ => {
                        if optional {
                            self.print("?.");
                        }
                        self.print("[");
                        self.find_expr(&computed.expr, true);
                        self.print("]");
                    }
                }
            }
        }
    }

    /// 成员访问的 receiver：链内嵌套的 OptChain 包装是 swc 的表示产物
    /// （V8 链内节点只带 optional 标记、无包装），透过包装继续渲染。
    fn visit_member_obj(&mut self, obj: &swc_ast::Expr) {
        let obj = strip_ts(obj);
        if let swc_ast::Expr::OptChain(chain) = obj {
            let before = self.prints;
            self.visit_opt_chain_inner(chain);
            if self.prints == before {
                self.print(INTERMEDIATE);
            }
            return;
        }
        self.find_expr(obj, true);
    }

    /// super.x：SuperPropertyReference 不打印任何内容，receiver 占位。
    fn visit_super_prop(&mut self, super_prop: &swc_ast::SuperPropExpr) {
        self.print(INTERMEDIATE);
        match &super_prop.prop {
            swc_ast::SuperProp::Ident(ident) => {
                self.print(".");
                self.print(ident.sym.as_ref());
            }
            swc_ast::SuperProp::Computed(computed) => {
                match fold_literal(strip_ts(&computed.expr)) {
                    Some(FoldedLit::Str(text)) => {
                        self.print(".");
                        self.print(&text);
                    }
                    _ => {
                        self.print("[");
                        self.find_expr(&computed.expr, true);
                        self.print("]");
                    }
                }
            }
        }
    }

    /// 可选链包装出现在渲染位置本身（非链内延续）时对应 V8 的
    /// VisitOptionalChain：print=false 转发 → 占位。链内延续（失败调用的
    /// callee、成员 receiver）由调用方经 [`Self::visit_opt_chain_inner`] 透传。
    fn visit_opt_chain(&mut self, chain: &swc_ast::OptChainExpr) {
        let _ = chain;
        self.print(INTERMEDIATE);
    }

    fn visit_opt_chain_inner(&mut self, chain: &swc_ast::OptChainExpr) {
        match chain.base.as_ref() {
            swc_ast::OptChainBase::Member(member) => self.visit_member(member, chain.optional),
            swc_ast::OptChainBase::Call(call) => {
                self.visit_call_callee(&call.callee);
                self.print("(...)");
            }
        }
    }

    /// 子位置的调用（非失败点）：渲染 callee 后打印 `(...)`，实参跳过。
    fn visit_sub_call(&mut self, call: &swc_ast::CallExpr) {
        match &call.callee {
            swc_ast::Callee::Expr(callee) => {
                self.visit_call_callee(callee);
                self.print("(...)");
            }
            swc_ast::Callee::Super(_) => {
                self.print("super");
                self.print("(...)");
            }
            swc_ast::Callee::Import(_) => {
                self.print("ImportCall(");
                for arg in &call.args {
                    self.find_expr(&arg.expr, true);
                }
                self.print(")");
            }
        }
    }

    /// 调用 callee 位置：swc 的 OptChain 包装若直接充当 callee（`o.foo?.()`
    /// 内层），对应 V8 链内节点，透过包装渲染。
    fn visit_call_callee(&mut self, callee: &swc_ast::Expr) {
        let callee = strip_ts(callee);
        if let swc_ast::Expr::OptChain(chain) = callee {
            let before = self.prints;
            self.visit_opt_chain_inner(chain);
            if self.prints == before {
                self.print(INTERMEDIATE);
            }
            return;
        }
        self.find_expr(callee, true);
    }

    /// 二元链：V8 对同优先级左结合链建 NaryOperation，整链渲染进一对括号；
    /// 混合运算符按结合结构嵌套括号。
    fn visit_bin(&mut self, bin: &swc_ast::BinExpr) {
        let mut operands: Vec<&swc_ast::Expr> = Vec::new();
        collect_nary_operands(bin, bin.op, &mut operands);
        self.print("(");
        for (index, operand) in operands.iter().enumerate() {
            if index > 0 {
                self.print(" ");
                self.print(bin.op.as_str());
                self.print(" ");
            }
            self.find_expr(operand, true);
        }
        self.print(")");
    }

    /// 赋值表达式：CallPrinter 在 print 模式只渲染赋值目标。
    fn visit_assign_target(&mut self, target: &swc_ast::AssignTarget) {
        match target {
            swc_ast::AssignTarget::Simple(simple) => match simple {
                swc_ast::SimpleAssignTarget::Ident(ident) => {
                    self.print(ident.sym.as_ref());
                }
                swc_ast::SimpleAssignTarget::Member(member) => self.visit_member(member, false),
                swc_ast::SimpleAssignTarget::SuperProp(super_prop) => {
                    self.visit_super_prop(super_prop);
                }
                swc_ast::SimpleAssignTarget::Paren(paren) => self.find_expr(&paren.expr, true),
                swc_ast::SimpleAssignTarget::OptChain(chain) => {
                    let before = self.prints;
                    self.visit_opt_chain_inner(chain);
                    if self.prints == before {
                        self.print(INTERMEDIATE);
                    }
                }
                swc_ast::SimpleAssignTarget::TsAs(ts_as) => self.find_expr(&ts_as.expr, true),
                swc_ast::SimpleAssignTarget::TsNonNull(ts_non_null) => {
                    self.find_expr(&ts_non_null.expr, true);
                }
                swc_ast::SimpleAssignTarget::TsSatisfies(ts_satisfies) => {
                    self.find_expr(&ts_satisfies.expr, true);
                }
                swc_ast::SimpleAssignTarget::TsTypeAssertion(ts_assertion) => {
                    self.find_expr(&ts_assertion.expr, true);
                }
                swc_ast::SimpleAssignTarget::TsInstantiation(ts_instantiation) => {
                    self.find_expr(&ts_instantiation.expr, true);
                }
                swc_ast::SimpleAssignTarget::Invalid(_) => self.print(INTERMEDIATE),
            },
            swc_ast::AssignTarget::Pat(pat) => match pat {
                // 解构目标即 V8 的 Object/ArrayLiteral 渲染。
                swc_ast::AssignTargetPat::Object(object) => {
                    self.print("{");
                    for _ in &object.props {
                        self.print(INTERMEDIATE);
                    }
                    self.print("}");
                }
                swc_ast::AssignTargetPat::Array(array) => {
                    self.print("[");
                    for (index, _) in array.elems.iter().enumerate() {
                        if index > 0 {
                            self.print(",");
                        }
                        self.print(INTERMEDIATE);
                    }
                    self.print("]");
                }
                swc_ast::AssignTargetPat::Invalid(_) => self.print(INTERMEDIATE),
            },
        }
    }

    /// 函数字面量：FindStatements 对每条体语句 print=false 占位；空体不打印，
    /// 由外层 Find 补一个占位。
    fn visit_fn_body(&mut self, statements: usize) {
        for _ in 0..statements {
            self.print(INTERMEDIATE);
        }
    }

    /// 类字面量：extends 与每个带值成员各占位一次（VisitClassLiteral 只
    /// Find 成员的 value）。
    fn visit_class(&mut self, class: &swc_ast::Class) {
        if class.super_class.is_some() {
            self.print(INTERMEDIATE);
        }
        for member in &class.body {
            let has_value = match member {
                swc_ast::ClassMember::Method(_)
                | swc_ast::ClassMember::PrivateMethod(_)
                | swc_ast::ClassMember::Constructor(_) => true,
                swc_ast::ClassMember::ClassProp(prop) => prop.value.is_some(),
                swc_ast::ClassMember::PrivateProp(prop) => prop.value.is_some(),
                _ => false,
            };
            if has_value {
                self.print(INTERMEDIATE);
            }
        }
    }

    /// 数组字面量：元素以逗号分隔逐个渲染，spread 为 `(...expr)`，
    /// 洞（the hole 字面量）打印不出内容 → 占位。
    fn visit_array(&mut self, array: &swc_ast::ArrayLit) {
        self.print("[");
        for (index, element) in array.elems.iter().enumerate() {
            if index > 0 {
                self.print(",");
            }
            match element {
                Some(item) if item.spread.is_some() => {
                    self.print("(...");
                    self.find_expr(&item.expr, true);
                    self.print(")");
                }
                Some(item) => self.find_expr(&item.expr, true),
                None => self.print(INTERMEDIATE),
            }
        }
        self.print("]");
    }

    /// PrintLiteral：字符串按 quote 决定引号，数字走 NumberToString。
    fn print_lit(&mut self, lit: &FoldedLit, quote: bool) {
        match lit {
            FoldedLit::Num(value) => self.print(&js_number_to_string(*value)),
            FoldedLit::Bool(value) => self.print(if *value { "true" } else { "false" }),
            FoldedLit::Null => self.print("null"),
            FoldedLit::Str(text) => {
                if quote {
                    self.print(&format!("\"{text}\""));
                } else {
                    self.print(text);
                }
            }
            // BigInt：PrintLiteral 无分支命中，不打印。
            FoldedLit::BigInt => {}
        }
    }
}

/// TS 类型包装（as/非空断言/类型断言/satisfies/泛型实例化）对 V8 不可见，
/// 渲染前透明剥离。括号不在此剥：可选链的括号是链边界（见 visit_expr）。
fn strip_ts(expr: &swc_ast::Expr) -> &swc_ast::Expr {
    match expr {
        swc_ast::Expr::TsAs(ts_as) => strip_ts(&ts_as.expr),
        swc_ast::Expr::TsNonNull(ts_non_null) => strip_ts(&ts_non_null.expr),
        swc_ast::Expr::TsTypeAssertion(ts_assertion) => strip_ts(&ts_assertion.expr),
        swc_ast::Expr::TsConstAssertion(ts_const) => strip_ts(&ts_const.expr),
        swc_ast::Expr::TsSatisfies(ts_satisfies) => strip_ts(&ts_satisfies.expr),
        swc_ast::Expr::TsInstantiation(ts_instantiation) => strip_ts(&ts_instantiation.expr),
        _ => expr,
    }
}

/// 同优先级左结合链收集为 nary 操作数序列：左子树同运算符且自身不可整体
/// 折叠时继续展开（可整体折叠的左子树在解析期已成为单个字面量操作数）。
fn collect_nary_operands<'a>(
    bin: &'a swc_ast::BinExpr,
    op: swc_ast::BinaryOp,
    operands: &mut Vec<&'a swc_ast::Expr>,
) {
    let left = strip_ts(&bin.left);
    let nary_op = is_nary_op(op);
    if nary_op
        && let swc_ast::Expr::Bin(left_bin) = left
        && left_bin.op == op
        && fold_literal(left).is_none()
    {
        collect_nary_operands(left_bin, op, operands);
    } else {
        operands.push(&bin.left);
    }
    operands.push(&bin.right);
}

/// V8 对哪些运算符做 nary 折叠（左结合二元；`**` 右结合、比较/in/instanceof
/// 不折叠成 nary）。
fn is_nary_op(op: swc_ast::BinaryOp) -> bool {
    matches!(
        op,
        swc_ast::BinaryOp::LogicalOr
            | swc_ast::BinaryOp::LogicalAnd
            | swc_ast::BinaryOp::NullishCoalescing
            | swc_ast::BinaryOp::BitOr
            | swc_ast::BinaryOp::BitXor
            | swc_ast::BinaryOp::BitAnd
            | swc_ast::BinaryOp::LShift
            | swc_ast::BinaryOp::RShift
            | swc_ast::BinaryOp::ZeroFillRShift
            | swc_ast::BinaryOp::Add
            | swc_ast::BinaryOp::Sub
            | swc_ast::BinaryOp::Mul
            | swc_ast::BinaryOp::Div
            | swc_ast::BinaryOp::Mod
    )
}

/// V8 解析期字面量折叠（含括号/TS 包装剥离、无插值模板 → 字符串）。
/// 返回 Some 表示该表达式在 V8 AST 中就是一个 Literal 节点。
fn fold_literal(expr: &swc_ast::Expr) -> Option<FoldedLit> {
    match strip_ts(expr) {
        swc_ast::Expr::Paren(paren) => fold_literal(&paren.expr),
        swc_ast::Expr::Lit(lit) => match lit {
            swc_ast::Lit::Num(num) => Some(FoldedLit::Num(num.value)),
            swc_ast::Lit::Bool(value) => Some(FoldedLit::Bool(value.value)),
            swc_ast::Lit::Null(_) => Some(FoldedLit::Null),
            swc_ast::Lit::Str(text) => {
                Some(FoldedLit::Str(text.value.to_string_lossy().into_owned()))
            }
            swc_ast::Lit::BigInt(_) => Some(FoldedLit::BigInt),
            // RegExpLiteral 在 V8 中不是 Literal 节点，不参与折叠。
            swc_ast::Lit::Regex(_) | swc_ast::Lit::JSXText(_) => None,
        },
        swc_ast::Expr::Tpl(tpl) if tpl.exprs.is_empty() => {
            // 无插值模板在 V8 解析期即字符串字面量。
            let cooked = tpl.quasis.first()?.cooked.as_ref()?;
            Some(FoldedLit::Str(cooked.to_string_lossy().into_owned()))
        }
        swc_ast::Expr::Unary(unary) => fold_unary(unary),
        swc_ast::Expr::Bin(bin) => fold_binary(bin),
        _ => None,
    }
}

/// 一元折叠（V8 BuildUnaryExpression）：`!` 折叠一切字面量为布尔；
/// `-`/`~` 只折叠数字字面量；`+` 对数字字面量返回原字面量。
fn fold_unary(unary: &swc_ast::UnaryExpr) -> Option<FoldedLit> {
    let operand = fold_literal(&unary.arg)?;
    match unary.op {
        swc_ast::UnaryOp::Bang => Some(FoldedLit::Bool(!lit_to_boolean(&operand))),
        swc_ast::UnaryOp::Minus => match operand {
            FoldedLit::Num(value) => Some(FoldedLit::Num(-value)),
            _ => None,
        },
        swc_ast::UnaryOp::Plus => match operand {
            FoldedLit::Num(value) => Some(FoldedLit::Num(value)),
            _ => None,
        },
        swc_ast::UnaryOp::Tilde => match operand {
            FoldedLit::Num(value) => Some(FoldedLit::Num(f64::from(!js_to_int32(value)))),
            _ => None,
        },
        _ => None,
    }
}

/// 二元数字折叠（V8 ShortcutNumericLiteralBinaryExpression）：两侧都是
/// 数字字面量时按 JS 数值语义折叠；比较运算符不折叠。
fn fold_binary(bin: &swc_ast::BinExpr) -> Option<FoldedLit> {
    let FoldedLit::Num(lhs) = fold_literal(&bin.left)? else {
        return None;
    };
    let FoldedLit::Num(rhs) = fold_literal(&bin.right)? else {
        return None;
    };
    let value = match bin.op {
        swc_ast::BinaryOp::Add => lhs + rhs,
        swc_ast::BinaryOp::Sub => lhs - rhs,
        swc_ast::BinaryOp::Mul => lhs * rhs,
        swc_ast::BinaryOp::Div => lhs / rhs,
        swc_ast::BinaryOp::Mod => lhs % rhs,
        swc_ast::BinaryOp::Exp => lhs.powf(rhs),
        swc_ast::BinaryOp::BitOr => f64::from(js_to_int32(lhs) | js_to_int32(rhs)),
        swc_ast::BinaryOp::BitAnd => f64::from(js_to_int32(lhs) & js_to_int32(rhs)),
        swc_ast::BinaryOp::BitXor => f64::from(js_to_int32(lhs) ^ js_to_int32(rhs)),
        swc_ast::BinaryOp::LShift => {
            f64::from(js_to_int32(lhs).wrapping_shl(js_to_uint32(rhs) & 0x1f))
        }
        swc_ast::BinaryOp::RShift => {
            f64::from(js_to_int32(lhs).wrapping_shr(js_to_uint32(rhs) & 0x1f))
        }
        swc_ast::BinaryOp::ZeroFillRShift => {
            f64::from(js_to_uint32(lhs).wrapping_shr(js_to_uint32(rhs) & 0x1f))
        }
        _ => return None,
    };
    Some(FoldedLit::Num(value))
}

/// 字面量的 ToBoolean（`!` 折叠用）。
fn lit_to_boolean(lit: &FoldedLit) -> bool {
    match lit {
        FoldedLit::Num(value) => *value != 0.0 && !value.is_nan(),
        FoldedLit::Bool(value) => *value,
        FoldedLit::Null => false,
        FoldedLit::Str(text) => !text.is_empty(),
        // `!0n` 罕见且 V8 对 BigInt 字面量的 ToBoolean 折叠同样成立，
        // 但保守放弃折叠（渲染回退占位）。
        FoldedLit::BigInt => false,
    }
}

/// ECMAScript ToInt32。
fn js_to_int32(value: f64) -> i32 {
    js_to_uint32(value) as i32
}

/// ECMAScript ToUint32。
fn js_to_uint32(value: f64) -> u32 {
    if !value.is_finite() || value == 0.0 {
        return 0;
    }
    let modulo = value.trunc().rem_euclid(4_294_967_296.0);
    modulo as u32
}

/// ECMAScript NumberToString（§6.1.6.1.20 子集）：语义层静态渲染专用，与
/// 宿主 `format_number_js` 同规则（±0 → "0"、|x|≥1e21 或 <1e-6 走规范化
/// 指数形态、其余取 Rust 最短往返十进制）。语义层不依赖 builtins crate
/// （编译器前端不引运行时依赖），故按 `js_number_property_key` 先例本地实现。
fn js_number_to_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if value == 0.0 {
        return "0".into();
    }
    let abs = value.abs();
    if abs >= 1e21 || abs < 1e-6 {
        // JS 指数形态：`1e+21`/`1.5e-7`（指数为正带 `+` 号）。
        let raw = format!("{value:e}");
        if let Some(pos) = raw.find('e') {
            let exponent: i32 = raw[pos + 1..].parse().unwrap_or(0);
            let sign = if exponent >= 0 { "+" } else { "" };
            return format!("{}e{sign}{exponent}", &raw[..pos]);
        }
        return raw;
    }
    format!("{value}")
}

/// RegExp 字面量 flags 的 V8 规范顺序（REGEXP_FLAG_LIST）。
fn canonical_regexp_flags(flags: &str) -> String {
    const ORDER: &[char] = &['d', 'g', 'i', 'm', 's', 'u', 'v', 'y'];
    ORDER.iter().filter(|c| flags.contains(**c)).collect()
}
