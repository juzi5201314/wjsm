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

/// 用于诊断的 ClassMember 源码区间。
pub(super) fn class_member_span(member: &swc_ast::ClassMember) -> Span {
    match member {
        swc_ast::ClassMember::Constructor(c) => c.span,
        swc_ast::ClassMember::Method(m) => m.span,
        swc_ast::ClassMember::PrivateMethod(m) => m.span,
        swc_ast::ClassMember::ClassProp(p) => p.span,
        swc_ast::ClassMember::PrivateProp(p) => p.span,
        swc_ast::ClassMember::StaticBlock(b) => b.span,
        swc_ast::ClassMember::TsIndexSignature(t) => t.span,
        swc_ast::ClassMember::Empty(e) => e.span,
        swc_ast::ClassMember::AutoAccessor(a) => a.span,
    }
}
/// 用于错误消息的 ClassMember 变体名称。
pub(super) fn class_member_kind(member: &swc_ast::ClassMember) -> &'static str {
    match member {
        swc_ast::ClassMember::Constructor(_) => "constructor",
        swc_ast::ClassMember::Method(_) => "method",
        swc_ast::ClassMember::PrivateMethod(_) => "private method",
        swc_ast::ClassMember::ClassProp(_) => "class property",
        swc_ast::ClassMember::PrivateProp(_) => "private property",
        swc_ast::ClassMember::StaticBlock(_) => "static block",
        swc_ast::ClassMember::TsIndexSignature(_) => "index signature",
        swc_ast::ClassMember::Empty(_) => "empty",
        swc_ast::ClassMember::AutoAccessor(_) => "auto accessor",
    }
}

/// 类私有方法 / 访问器在 lowering 阶段的绑定描述。
pub(crate) enum PrivateMemberKind {
    Method(FunctionId),
    Accessor {
        getter: Option<FunctionId>,
        setter: Option<FunctionId>,
    },
}

impl Lowerer {
    fn push_class_private_name_scope(&mut self, body: &[swc_ast::ClassMember]) {
        let class_private_id = self.next_private_name_id;
        self.next_private_name_id += 1;
        let mut names = std::collections::HashMap::new();
        for member in body {
            let source_name = match member {
                swc_ast::ClassMember::PrivateMethod(method) => method.key.name.to_string(),
                swc_ast::ClassMember::PrivateProp(prop) => prop.key.name.to_string(),
                _ => continue,
            };
            names
                .entry(source_name.clone())
                .or_insert_with(|| format!("#{source_name}@{class_private_id}"));
        }
        self.private_name_stack.push(names);
    }

    fn pop_class_private_name_scope(&mut self) {
        self.private_name_stack.pop();
    }

    pub(crate) fn resolve_private_storage_name(
        &self,
        source_name: &str,
        span: Span,
    ) -> Result<String, LoweringError> {
        self.private_name_stack
            .iter()
            .rev()
            .find_map(|scope| scope.get(source_name))
            .cloned()
            .ok_or_else(|| {
                self.error(
                    span,
                    format!("Private field '#{source_name}' is not declared"),
                )
            })
    }

    /// 将类构造器 IR 函数物化为运行时可调用值：无捕获时为 FunctionRef，有捕获时为 CreateClosure + 共享 env。
    fn materialize_constructor_value(
        &mut self,
        block: BasicBlockId,
        function_id: FunctionId,
        captured: &[CapturedBinding],
        span: Span,
    ) -> Result<ValueId, LoweringError> {
        let func_ref_const = self.module.add_constant(Constant::FunctionRef(function_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );
        if captured.is_empty() {
            return Ok(func_ref_val);
        }
        let mut closure_block = block;
        let env_val = self.ensure_shared_env(closure_block, captured, span)?;
        closure_block = self.resolve_store_block(closure_block);
        let closure_val = self.alloc_value();
        self.current_function.append_instruction(
            closure_block,
            Instruction::CallBuiltin {
                dest: Some(closure_val),
                builtin: Builtin::CreateClosure,
                args: vec![func_ref_val, env_val],
            },
        );
        Ok(closure_val)
    }

    fn emit_string_const(&mut self, block: BasicBlockId, value: &str) -> ValueId {
        let constant = self
            .module
            .add_constant(Constant::String(value.to_string()));
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        dest
    }

    fn emit_undefined_const(&mut self, block: BasicBlockId) -> ValueId {
        let constant = self.module.add_constant(Constant::Undefined);
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        dest
    }

    fn emit_bool_const(&mut self, block: BasicBlockId, value: bool) -> ValueId {
        let constant = self.module.add_constant(Constant::Bool(value));
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        dest
    }

    fn emit_context_prop(
        &mut self,
        block: BasicBlockId,
        context: ValueId,
        key: &str,
        value: ValueId,
    ) {
        let key = self.emit_string_const(block, key);
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: context,
                key,
                value,
            },
        );
    }

    fn emit_decorator_context(
        &mut self,
        block: BasicBlockId,
        kind: &str,
        name: Option<&str>,
        is_static: Option<bool>,
        is_private: Option<bool>,
    ) -> ValueId {
        let context = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: context,
                capacity: 5,
            },
        );

        let kind = self.emit_string_const(block, kind);
        self.emit_context_prop(block, context, "kind", kind);

        let name = name
            .map(|name| self.emit_string_const(block, name))
            .unwrap_or_else(|| self.emit_undefined_const(block));
        self.emit_context_prop(block, context, "name", name);

        if let Some(is_static) = is_static {
            let value = self.emit_bool_const(block, is_static);
            self.emit_context_prop(block, context, "static", value);
        }
        if let Some(is_private) = is_private {
            let value = self.emit_bool_const(block, is_private);
            self.emit_context_prop(block, context, "private", value);
        }

        context
    }

    fn emit_decorator_result_or_original(
        &mut self,
        block: BasicBlockId,
        original: ValueId,
        result: ValueId,
    ) -> (BasicBlockId, ValueId) {
        let undefined = self.emit_undefined_const(block);
        let has_replacement = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Compare {
                dest: has_replacement,
                op: CompareOp::StrictNotEq,
                lhs: result,
                rhs: undefined,
            },
        );

        let replacement_block = self.current_function.new_block();
        let original_block = self.current_function.new_block();
        let merge_block = self.current_function.new_block();
        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition: has_replacement,
                true_block: replacement_block,
                false_block: original_block,
            },
        );
        self.current_function.set_terminator(
            replacement_block,
            Terminator::Jump {
                target: merge_block,
            },
        );
        self.current_function.set_terminator(
            original_block,
            Terminator::Jump {
                target: merge_block,
            },
        );

        let value = self.alloc_value();
        self.current_function.append_instruction(
            merge_block,
            Instruction::Phi {
                dest: value,
                sources: vec![
                    PhiSource {
                        predecessor: replacement_block,
                        value: result,
                    },
                    PhiSource {
                        predecessor: original_block,
                        value: original,
                    },
                ],
            },
        );
        (merge_block, value)
    }

    fn emit_apply_class_decorators(
        &mut self,
        mut block: BasicBlockId,
        mut class_value: ValueId,
        decorators: &[swc_ast::Decorator],
        class_name: Option<&str>,
    ) -> Result<(BasicBlockId, ValueId), LoweringError> {
        let mut decorator_values = Vec::with_capacity(decorators.len());
        for decorator in decorators {
            let value = self.lower_expr(&decorator.expr, block)?;
            block = self.resolve_store_block(block);
            decorator_values.push(value);
        }

        for decorator in decorator_values.into_iter().rev() {
            let context = self.emit_decorator_context(block, "class", class_name, None, None);
            let result = self.alloc_value();
            let this_val = self.emit_undefined_const(block);
            self.current_function.append_instruction(
                block,
                Instruction::Call {
                    dest: Some(result),
                    callee: decorator,
                    this_val,
                    args: vec![class_value, context],
                },
            );
            (block, class_value) =
                self.emit_decorator_result_or_original(block, class_value, result);
        }

        Ok((block, class_value))
    }

    fn emit_apply_value_decorators(
        &mut self,
        mut block: BasicBlockId,
        mut original: ValueId,
        decorators: &[swc_ast::Decorator],
        kind: &str,
        name: &str,
        is_static: bool,
        is_private: bool,
    ) -> Result<(BasicBlockId, ValueId), LoweringError> {
        let mut decorator_values = Vec::with_capacity(decorators.len());
        for decorator in decorators {
            let value = self.lower_expr(&decorator.expr, block)?;
            block = self.resolve_store_block(block);
            decorator_values.push(value);
        }

        for decorator in decorator_values.into_iter().rev() {
            let context = self.emit_decorator_context(
                block,
                kind,
                Some(name),
                Some(is_static),
                Some(is_private),
            );
            let result = self.alloc_value();
            let this_val = self.emit_undefined_const(block);
            self.current_function.append_instruction(
                block,
                Instruction::Call {
                    dest: Some(result),
                    callee: decorator,
                    this_val,
                    args: vec![original, context],
                },
            );
            (block, original) = self.emit_decorator_result_or_original(block, original, result);
        }

        Ok((block, original))
    }

    fn emit_instance_initializers(
        &mut self,
        mut block: BasicBlockId,
        this_scope_id: usize,
        members: &[swc_ast::ClassMember],
        private_members: &[(String, bool, PrivateMemberKind)],
    ) -> Result<BasicBlockId, LoweringError> {
        for member in members {
            match member {
                swc_ast::ClassMember::PrivateProp(prop) if !prop.is_static => {
                    let field_name =
                        self.resolve_private_storage_name(prop.key.name.as_ref(), prop.key.span)?;
                    block = self.emit_field_init(
                        block,
                        this_scope_id,
                        &field_name,
                        prop.value.as_deref(),
                        true,
                    )?;
                }
                swc_ast::ClassMember::ClassProp(prop) if !prop.is_static => {
                    block = self.emit_field_init_with_key(
                        block,
                        this_scope_id,
                        &prop.key,
                        prop.value.as_deref(),
                    )?;
                }
                swc_ast::ClassMember::Constructor(_)
                | swc_ast::ClassMember::Method(_)
                | swc_ast::ClassMember::PrivateMethod(_)
                | swc_ast::ClassMember::StaticBlock(_) => {}
                swc_ast::ClassMember::PrivateProp(p) if p.is_static => {}
                swc_ast::ClassMember::ClassProp(p) if p.is_static => {}
                other => {
                    return Err(self.error(
                        class_member_span(other),
                        format!(
                            "unsupported class member `{}` during instance field initialization",
                            class_member_kind(other),
                        ),
                    ));
                }
            }
        }

        for (field_name, is_static, kind) in private_members {
            if !is_static {
                let this_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: this_val,
                        name: format!("${this_scope_id}.$this"),
                    },
                );
                match kind {
                    PrivateMemberKind::Method(func_id) => {
                        self.emit_private_method_bind(block, this_val, field_name, *func_id);
                    }
                    PrivateMemberKind::Accessor { getter, setter } => {
                        self.emit_private_accessor_bind(
                            block, this_val, field_name, *getter, *setter,
                        );
                    }
                }
                block = self.resolve_store_block(block);
            }
        }

        Ok(block)
    }

    /// 收集类体中的私有方法/访问器并生成对应 IR 函数。
    fn collect_class_private_members(
        &mut self,
        class_name: &str,
        body: &[swc_ast::ClassMember],
    ) -> Result<Vec<(String, bool, PrivateMemberKind)>, LoweringError> {
        use std::collections::HashMap;
        let mut out: Vec<(String, bool, PrivateMemberKind)> = Vec::new();
        let mut accessor_pending: HashMap<
            (String, bool),
            (Option<FunctionId>, Option<FunctionId>),
        > = HashMap::new();

        for member in body {
            let swc_ast::ClassMember::PrivateMethod(pm) = member else {
                continue;
            };
            let field_name =
                self.resolve_private_storage_name(pm.key.name.as_ref(), pm.key.span)?;
            let is_static = pm.is_static;
            let accessor = matches!(
                pm.kind,
                swc_ast::MethodKind::Getter | swc_ast::MethodKind::Setter
            );
            let role = if matches!(pm.kind, swc_ast::MethodKind::Getter) {
                "get"
            } else if matches!(pm.kind, swc_ast::MethodKind::Setter) {
                "set"
            } else {
                ""
            };
            let fn_name = if accessor {
                if is_static {
                    format!("{}.static_{}_{}", class_name, role, pm.key.name)
                } else {
                    format!("{}.{}_{}", class_name, role, pm.key.name)
                }
            } else if is_static {
                format!("{}.static_{}", class_name, pm.key.name)
            } else {
                format!("{}.{}", class_name, pm.key.name)
            };

            self.push_function_context(&fn_name, BasicBlockId(0));
            self.is_method = true;
            self.super_allowed = true;
            self.set_lexical_home_object_for_enclosing_method(
                Self::PENDING_CTOR_FUNCTION_ID,
                is_static,
            );
            let env_scope_id = self
                .scopes
                .declare("$env", VarKind::Let, true)
                .map_err(|msg| self.error(pm.span, msg))?;
            let this_scope_id = self
                .scopes
                .declare("$this", VarKind::Let, true)
                .map_err(|msg| self.error(pm.span, msg))?;
            let mut param_ir_names = vec![
                format!("${env_scope_id}.$env"),
                format!("${this_scope_id}.$this"),
            ];
            for param in &pm.function.params {
                if let swc_ast::Pat::Ident(binding_ident) = &param.pat {
                    let name = binding_ident.id.sym.to_string();
                    let scope_id = self
                        .scopes
                        .declare(&name, VarKind::Let, true)
                        .map_err(|msg| self.error(pm.span, msg))?;
                    param_ir_names.push(format!("${scope_id}.{name}"));
                }
            }
            if let Some(body) = &pm.function.body {
                self.predeclare_block_stmts(&body.stmts)?;
            }
            let m_entry = BasicBlockId(0);
            self.emit_hoisted_var_initializers(m_entry);
            self.arguments_param_count = Self::count_regular_params(&pm.function.params);
            let m_entry = self.emit_arguments_init(
                m_entry,
                Self::function_needs_arguments_object(&pm.function),
            )?;
            self.eval_caller_has_arguments = Self::detect_param_arguments(&pm.function.params)
                || self.scopes.lookup("arguments").is_ok();
            let mut m_flow = StmtFlow::Open(m_entry);
            if let Some(body) = &pm.function.body {
                for stmt in &body.stmts {
                    if matches!(m_flow, StmtFlow::Terminated) {
                        continue;
                    }
                    m_flow = self.lower_stmt(stmt, m_flow)?;
                }
            }
            if let StmtFlow::Open(b) = m_flow {
                self.current_function
                    .set_terminator(b, Terminator::Return { value: None });
            }
            let m_old_fn = std::mem::replace(
                &mut self.current_function,
                FunctionBuilder::new("", BasicBlockId(0)),
            );
            let m_has_eval = m_old_fn.has_eval();
            let m_blocks = m_old_fn.into_blocks();
            let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
            m_ir_function.set_has_eval(m_has_eval);
            m_ir_function.set_params(param_ir_names);
            let m_captured = self.captured_names_stack.last().unwrap().clone();
            m_ir_function.set_captured_names(Self::captured_display_names(&m_captured));
            for b in m_blocks {
                m_ir_function.push_block(b);
            }
            let m_function_id = self.module.push_function(m_ir_function);
            self.pop_function_context();

            if accessor {
                let key = (field_name.clone(), is_static);
                let entry = accessor_pending.entry(key.clone()).or_insert((None, None));
                if matches!(pm.kind, swc_ast::MethodKind::Getter) {
                    entry.0 = Some(m_function_id);
                } else {
                    entry.1 = Some(m_function_id);
                }
                if entry.0.is_some() || entry.1.is_some() {
                    let (g, s) = entry.clone();
                    if let Some(pos) = out.iter().position(|(n, st, k)| {
                        n == &key.0
                            && *st == key.1
                            && matches!(k, PrivateMemberKind::Accessor { .. })
                    }) {
                        out[pos].2 = PrivateMemberKind::Accessor {
                            getter: g,
                            setter: s,
                        };
                    } else {
                        out.push((
                            key.0,
                            key.1,
                            PrivateMemberKind::Accessor {
                                getter: g,
                                setter: s,
                            },
                        ));
                    }
                }
            } else {
                out.push((
                    field_name,
                    is_static,
                    PrivateMemberKind::Method(m_function_id),
                ));
            }
        }
        Ok(out)
    }

    fn patch_private_member_home_objects(
        &mut self,
        ctor_function_id: FunctionId,
        private_members: &[(String, bool, PrivateMemberKind)],
    ) {
        for (_, is_static, kind) in private_members {
            let func_ids: Vec<FunctionId> = match kind {
                PrivateMemberKind::Method(id) => vec![*id],
                PrivateMemberKind::Accessor { getter, setter } => {
                    getter.iter().chain(setter).copied().collect()
                }
            };
            for func_id in func_ids {
                if let Some(function) = self.module.function_mut(func_id) {
                    function.home_object = Some(if *is_static {
                        HomeObject::Constructor(ctor_function_id)
                    } else {
                        HomeObject::Prototype(ctor_function_id)
                    });
                }
            }
        }
    }

    fn emit_static_private_member_binds(
        &mut self,
        block: BasicBlockId,
        ctor_dest: ValueId,
        private_members: &[(String, bool, PrivateMemberKind)],
    ) {
        for (field_name, is_static, kind) in private_members {
            if !*is_static {
                continue;
            }
            match kind {
                PrivateMemberKind::Method(func_id) => {
                    self.emit_private_method_bind(block, ctor_dest, field_name, *func_id);
                }
                PrivateMemberKind::Accessor { getter, setter } => {
                    self.emit_private_accessor_bind(block, ctor_dest, field_name, *getter, *setter);
                }
            }
        }
    }
}

mod decl;
mod expr;
