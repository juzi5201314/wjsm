use super::*;
use swc_core::common::Span;
use swc_core::ecma::visit::{Visit, VisitWith};

#[derive(Default)]
struct DerivedCtorPreSuperUse {
    seen_super_call: bool,
    invalid_span: Option<Span>,
}

impl Visit for DerivedCtorPreSuperUse {
    fn visit_call_expr(&mut self, call: &swc_ast::CallExpr) {
        if self.invalid_span.is_some() {
            return;
        }
        if matches!(call.callee, swc_ast::Callee::Super(_)) {
            for arg in &call.args {
                arg.visit_with(self);
            }
            self.seen_super_call = true;
            return;
        }
        call.visit_children_with(self);
    }

    fn visit_this_expr(&mut self, this_expr: &swc_ast::ThisExpr) {
        if !self.seen_super_call && self.invalid_span.is_none() {
            self.invalid_span = Some(this_expr.span);
        }
    }

    fn visit_super_prop_expr(&mut self, super_prop: &swc_ast::SuperPropExpr) {
        if !self.seen_super_call && self.invalid_span.is_none() {
            self.invalid_span = Some(super_prop.span);
        }
    }

    fn visit_function(&mut self, _: &swc_ast::Function) {}

    fn visit_arrow_expr(&mut self, arrow: &swc_ast::ArrowExpr) {
        if self.invalid_span.is_some() {
            return;
        }
        // 箭头函数词法捕获外层 this；派生构造器 super() 前禁止访问该 this。
        arrow.visit_children_with(self);
    }

    fn visit_class(&mut self, _: &swc_ast::Class) {}
}

pub(super) fn first_pre_super_this_or_super_span(body: &swc_ast::BlockStmt) -> Option<Span> {
    let mut visitor = DerivedCtorPreSuperUse::default();
    body.visit_with(&mut visitor);
    visitor.invalid_span
}
pub(super) fn stmt_is_direct_super_call(stmt: &swc_ast::Stmt) -> bool {
    matches!(
        stmt,
        swc_ast::Stmt::Expr(expr_stmt)
            if matches!(
                expr_stmt.expr.as_ref(),
                swc_ast::Expr::Call(call)
                    if matches!(call.callee, swc_ast::Callee::Super(_))
            )
    )
}

/// 私有名静态校验（早错误）：
/// 1. AllPrivateIdentifiersValid（ES §13.3.1.1）：任何 `obj.#x` / `#x in obj` 引用都必须
///    出现在声明 `#x` 的某个词法封闭类内，否则为 SyntaxError。
/// 2. ClassBody 私有名重复：同一类体内私有名不得重复声明（同名 getter+setter 各一次的
///    配对除外），否则为 SyntaxError。
///
/// 作为降级前的一次性 AST 遍历执行（模式同 `DerivedCtorPreSuperUse`）。
struct PrivateNameValidator {
    /// 词法作用域栈：进入每个类体时压入其声明的全部私有名集合；引用有效当且仅当其名
    /// 存在于栈中任一层（最近的或更外层的封闭类）。
    scopes: Vec<std::collections::HashSet<String>>,
    error: Option<(Span, String)>,
}

impl PrivateNameValidator {
    fn new() -> Self {
        Self {
            scopes: Vec::new(),
            error: None,
        }
    }

    /// 收集类体声明的全部私有名，并检测重复声明。返回该类的私有名集合。
    fn collect_class_private_names(
        &mut self,
        class: &swc_ast::Class,
    ) -> std::collections::HashSet<String> {
        use std::collections::HashMap;
        // 每个私有名累计 (值/普通方法 计数, getter 计数, setter 计数)。
        let mut tally: HashMap<String, (u32, u32, u32)> = HashMap::new();
        let mut order: Vec<(String, Span)> = Vec::new();
        for member in &class.body {
            let (name, span, slot) = match member {
                swc_ast::ClassMember::PrivateMethod(m) => {
                    let slot = match m.kind {
                        swc_ast::MethodKind::Getter => 1usize,
                        swc_ast::MethodKind::Setter => 2usize,
                        swc_ast::MethodKind::Method => 0usize,
                    };
                    (m.key.name.to_string(), m.key.span, slot)
                }
                swc_ast::ClassMember::PrivateProp(p) => {
                    (p.key.name.to_string(), p.key.span, 0usize)
                }
                _ => continue,
            };
            let entry = tally.entry(name.clone()).or_insert((0, 0, 0));
            match slot {
                0 => entry.0 += 1,
                1 => entry.1 += 1,
                _ => entry.2 += 1,
            }
            order.push((name, span));
        }
        // 重复规则：非访问器名只能出现一次且不可与访问器同名；getter / setter 各至多一次。
        if self.error.is_none() {
            for (name, span) in &order {
                let (values, getters, setters) = tally[name];
                let duplicate = values > 1
                    || (values >= 1 && getters + setters > 0)
                    || getters > 1
                    || setters > 1;
                if duplicate {
                    self.error = Some((
                        *span,
                        format!("Identifier '#{name}' has already been declared"),
                    ));
                    break;
                }
            }
        }
        tally.into_keys().collect()
    }
}

impl Visit for PrivateNameValidator {
    fn visit_class(&mut self, class: &swc_ast::Class) {
        let names = self.collect_class_private_names(class);
        self.scopes.push(names);
        class.visit_children_with(self);
        self.scopes.pop();
    }

    fn visit_private_name(&mut self, name: &swc_ast::PrivateName) {
        // 引用（含类体内的声明键）：声明键此时已在作用域内，故仅词法外的引用会报错。
        if self.error.is_none()
            && !self
                .scopes
                .iter()
                .any(|scope| scope.contains(name.name.as_ref()))
        {
            self.error = Some((
                name.span,
                format!(
                    "Private field '#{}' must be declared in an enclosing class",
                    name.name
                ),
            ));
        }
    }
}

/// 对整棵模块 AST 运行私有名静态校验，返回首个早错误（若有）。
pub(crate) fn validate_private_names(module: &swc_ast::Module) -> Result<(), LoweringError> {
    let mut validator = PrivateNameValidator::new();
    module.visit_with(&mut validator);
    if let Some((span, message)) = validator.error {
        return Err(LoweringError::Diagnostic(Diagnostic::new(
            span.lo.0, span.hi.0, message,
        )));
    }
    Ok(())
}

impl Lowerer {
    fn emit_instance_initializers(
        &mut self,
        mut block: BasicBlockId,
        this_scope_id: usize,
        members: &[swc_ast::ClassMember],
        private_method_ids: &[(String, bool, FunctionId)],
    ) -> Result<BasicBlockId, LoweringError> {
        for member in members {
            match member {
                swc_ast::ClassMember::PrivateProp(prop) if !prop.is_static => {
                    let field_name = format!("#{}", prop.key.name);
                    block = self.emit_field_init(
                        block,
                        this_scope_id,
                        &field_name,
                        prop.value.as_deref(),
                        true,
                    )?;
                }
                swc_ast::ClassMember::ClassProp(prop) if !prop.is_static => {
                    let prop_name = match &prop.key {
                        swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
                        swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
                        swc_ast::PropName::Num(n) => n.value.to_string(),
                        _ => continue,
                    };
                    block = self.emit_field_init(
                        block,
                        this_scope_id,
                        &prop_name,
                        prop.value.as_deref(),
                        false,
                    )?;
                }
                _ => {}
            }
        }

        for (field_name, is_static, func_id) in private_method_ids {
            if !is_static {
                let this_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: this_val,
                        name: format!("${this_scope_id}.$this"),
                    },
                );
                self.emit_private_method_bind(block, this_val, field_name, *func_id);
                block = self.resolve_store_block(block);
            }
        }

        Ok(block)
    }
}

mod decl;
mod expr;
