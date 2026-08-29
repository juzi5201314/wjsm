use super::*;

/// FieldDefinition 早错误检测（ES §15.7.1 ContainsArguments）：字段初始化器
/// 不得引用 `arguments`。检测穿透箭头函数（词法透明），止于普通函数 /
/// 构造器 / 对象访问器体（它们自行绑定 `arguments`）；TS 类型位置整体跳过
/// （类型擦除，不参与求值）。V8 / SpiderMonkey 同样拒绝初始化器内的
/// `arguments:` 标签形式，这里与真实引擎保持一致。
#[derive(Default)]
struct ContainsArguments {
    span: Option<Span>,
}

impl Visit for ContainsArguments {
    fn visit_ident(&mut self, ident: &swc_ast::Ident) {
        if self.span.is_none() && ident.sym == "arguments" {
            self.span = Some(ident.span);
        }
    }

    fn visit_function(&mut self, _: &swc_ast::Function) {}

    fn visit_constructor(&mut self, _: &swc_ast::Constructor) {}

    fn visit_getter_prop(&mut self, prop: &swc_ast::GetterProp) {
        prop.key.visit_with(self);
    }

    fn visit_setter_prop(&mut self, prop: &swc_ast::SetterProp) {
        prop.key.visit_with(self);
    }

    fn visit_ts_type(&mut self, _: &swc_ast::TsType) {}
}

/// 初始化器内首个 `arguments` 引用的位置（无引用时为 None）。
pub(super) fn field_initializer_arguments_span(init: &swc_ast::Expr) -> Option<Span> {
    let mut visitor = ContainsArguments::default();
    init.visit_with(&mut visitor);
    visitor.span
}

/// 单个静态字段的降级参数（键已在类定义期第一遍求值完成）。
pub(super) struct StaticFieldInit<'a> {
    pub(super) class_name: &'a str,
    pub(super) ctor_function_id: FunctionId,
    pub(super) ctor_dest: ValueId,
    pub(super) key_dest: ValueId,
    pub(super) init_value: Option<&'a swc_ast::Expr>,
    pub(super) is_private: bool,
    pub(super) span: Span,
    pub(super) init_index: u32,
}

impl Lowerer {
    /// FieldDefinition 早错误（ES §15.7.1）：初始化器包含 `arguments` 即
    /// SyntaxError（静态与实例字段同一条款，消息对齐 V8/Node）。
    pub(super) fn check_field_initializer_arguments(
        &self,
        init: Option<&swc_ast::Expr>,
    ) -> Result<(), LoweringError> {
        let Some(init) = init else {
            return Ok(());
        };
        if let Some(span) = field_initializer_arguments_span(init) {
            return Err(self.error(
                span,
                "'arguments' is not allowed in class field initializer or static initialization block",
            ));
        }
        Ok(())
    }

    /// 静态字段 DefineField（ES ClassDefinitionEvaluation 静态元素执行期）。
    ///
    /// 有初始化器时按规范以合成初始化器函数求值：`this` 为构造器本身、
    /// [[HomeObject]] 为构造器（`super.x` 沿父类解析）、new.target 为
    /// undefined，与 static block 同一接线；初始化器异常在类定义期传播。
    /// 无初始化器时直接定义 undefined（规范无合成函数）。随后
    /// CreateDataPropertyOrThrow / PrivateSet 完成属性定义。
    pub(super) fn lower_static_field_member(
        &mut self,
        block: BasicBlockId,
        field: &StaticFieldInit<'_>,
    ) -> Result<BasicBlockId, LoweringError> {
        self.check_field_initializer_arguments(field.init_value)?;
        // 调用方是否已按静态键 / 私有名暂存 NamedEvaluation 提示（提示随
        // 合成初始化器函数体的降级被消费，须在此先行记录）。
        let staged_named_eval = self.named_eval_hint.is_some();
        let mut block = block;
        let init_val = if let Some(init) = field.init_value {
            let fn_name = format!(
                "{}.static_field_init_{}",
                field.class_name, field.init_index
            );
            let function = self.lower_static_field_init_fn(
                &fn_name,
                init,
                field.span,
                field.ctor_function_id,
            )?;
            let (continuation, function_value) =
                self.materialize_class_function_value(block, &function, field.span)?;
            block = continuation;
            let value = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Call {
                    dest: Some(value),
                    callee: function_value,
                    this_val: field.ctor_dest,
                    args: vec![],
                    callsite: None,
                },
            );
            // DefineField 的 `? Call(initializer, receiver)`：初始化器抛出的
            // 异常必须在类定义期传播，不得流入属性定义。
            block = self.lower_value_exception_branch(block, value)?;
            // NamedEvaluation：计算键静态字段的匿名函数定义以实际键值
            // 运行时命名（私有 / 静态键由调用方以降级期提示命名）。
            if !field.is_private && !staged_named_eval && Self::is_anonymous_fn_definition(init) {
                self.emit_runtime_set_function_name(
                    block,
                    value,
                    field.key_dest,
                    AccessorPrefix::None,
                );
            }
            value
        } else {
            let constant = self.module.add_constant(Constant::Undefined);
            let dest = self.alloc_value();
            self.current_function
                .append_instruction(block, Instruction::Const { dest, constant });
            dest
        };
        if field.is_private {
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::PrivateSet,
                    args: vec![field.ctor_dest, field.key_dest, init_val],
                },
            );
            return Ok(self.resolve_store_block(block));
        }
        let result =
            self.emit_create_data_property(block, field.ctor_dest, field.key_dest, init_val);
        self.lower_value_exception_branch(block, result)
    }

    /// 为静态字段初始化器创建合成 IR 函数（体即 `return <initializer>`）。
    ///
    /// 接线与 static block 一致：方法语境 + 构造器 [[HomeObject]]，调用点以
    /// 构造器为 `this`。初始化器是单个表达式：无 var 提升，`arguments`
    /// 已被早错误拦截，无需物化 arguments 对象。
    fn lower_static_field_init_fn(
        &mut self,
        fn_name: &str,
        init: &swc_ast::Expr,
        span: Span,
        ctor_function_id: FunctionId,
    ) -> Result<LoweredClassFunction, LoweringError> {
        self.push_function_context(fn_name, BasicBlockId(0));
        // 类体代码恒为严格模式（ClassDefinitionEvaluation）。
        self.strict_mode = true;
        self.is_method = true;
        self.super_allowed = true;
        self.set_lexical_home_object_for_enclosing_method(ctor_function_id, true);

        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        self.arguments_param_count = 0;
        self.eval_caller_has_arguments = self.scopes.lookup("arguments").is_ok();

        let mut block = BasicBlockId(0);
        let value = self.lower_field_init_value(&mut block, Some(init))?;
        self.current_function
            .set_terminator(block, Terminator::Return { value: Some(value) });

        let function = self.finalize_class_method_function(
            fn_name,
            span,
            param_ir_names,
            HomeObject::Constructor(ctor_function_id),
        );
        self.pop_function_context();
        Ok(function)
    }
}
