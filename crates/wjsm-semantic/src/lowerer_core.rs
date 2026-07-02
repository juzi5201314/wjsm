use super::*;
use swc_core::ecma::visit::{Visit, VisitWith};

/// 扫描某函数体/形参是否引用了隐式 `arguments` 绑定。
///
/// 用于"`arguments` 对象惰性消除"优化：函数体未引用 `arguments` 时跳过
/// `emit_arguments_init`，使普通函数声明恢复为 no-GC（解锁 backend Layer 3 call-spill 省略）。
///
/// **边界语义（正确性红线）**：
/// - 嵌套**箭头函数**继承外层非箭头函数的 `arguments` → 必须下降（默认 `visit_*` 即下降）。
/// - 嵌套**普通函数 / 方法**有各自的 `arguments` → 在 `visit_function` 处截断（no-op）。
/// - 类的计算属性键 / `extends` / 字段初始化器在外层作用域求值 → 不覆写 `visit_class`，
///   保持默认下降（保守正确）；其内部方法体仍被 `visit_function` no-op 截断。
/// - 直接 `eval(...)` 可能动态读取 `arguments` → 保守视为引用。
///
/// **保守原则**：任何不确定都判定为"引用"（宁可多建 arguments 对象，不可漏建——
/// 漏建会在后续 body 降级时 `lookup("arguments")` 失败而编译报错，但本扫描的职责是避免误删）。
#[derive(Default)]
struct ArgumentsRefScan {
    found: bool,
}

impl Visit for ArgumentsRefScan {
    fn visit_ident(&mut self, ident: &swc_ast::Ident) {
        if ident.sym.as_ref() == "arguments" {
            self.found = true;
        }
    }

    fn visit_call_expr(&mut self, call: &swc_ast::CallExpr) {
        // 直接 eval 可能动态引用 arguments，保守保留。
        if let swc_ast::Callee::Expr(callee) = &call.callee
            && let swc_ast::Expr::Ident(ident) = callee.as_ref()
            && ident.sym.as_ref() == "eval"
        {
            self.found = true;
        }
        call.visit_children_with(self);
    }

    // 嵌套普通函数 / 方法拥有各自的 arguments，不下降。
    // （箭头函数走默认下降路径，因其继承外层 arguments。）
    fn visit_function(&mut self, _: &swc_ast::Function) {}
}

impl Lowerer {
    pub(crate) fn new() -> Self {
        let mut scopes = ScopeTree::new();
        // 预注册 ECMAScript 全局内置标识符
        let _ = scopes.declare("undefined", VarKind::Var, true);
        let _ = scopes.declare("NaN", VarKind::Var, true);
        let _ = scopes.declare("Infinity", VarKind::Var, true);

        Self {
            module: Module::new(),
            next_value: 0,
            scopes,
            hoisted_vars: Vec::new(),
            hoisted_vars_set: std::collections::HashSet::new(),
            current_function: FunctionBuilder::new(MODULE_ENTRY_IR_NAME, BasicBlockId(0)),
            label_stack: Vec::new(),
            finally_stack: Vec::new(),
            try_contexts: Vec::new(),
            next_temp: 0,
            pending_loop_label: None,
            active_finalizers: Vec::new(),
            anon_counter: 0,
            private_name_stack: Vec::new(),
            next_private_name_id: 0,
            function_stack: Vec::new(),
            function_hoisted_stack: Vec::new(),
            function_next_value_stack: Vec::new(),
            function_next_temp_stack: Vec::new(),
            async_context_stack: Vec::new(),
            function_try_contexts_stack: Vec::new(),
            function_finally_stack_stack: Vec::new(),
            function_label_stack_stack: Vec::new(),
            function_active_finalizers_stack: Vec::new(),
            function_pending_loop_label_stack: Vec::new(),
            captured_names_stack: Vec::new(),
            function_scope_id_stack: Vec::new(),
            is_arrow_fn_stack: Vec::new(),
            super_allowed: false,
            super_call_allowed: false,
            function_super_allowed_stack: Vec::new(),
            function_super_call_allowed_stack: Vec::new(),
            function_is_arrow_stack: Vec::new(),
            function_is_method_stack: Vec::new(),
            lexical_home_object: None,
            function_lexical_home_object_stack: Vec::new(),
            shared_env_stack: Vec::new(),
            current_module_id: None,
            import_bindings: std::collections::HashMap::new(),
            export_map: std::collections::HashMap::new(),
            import_aliases: std::collections::HashMap::new(),
            module_scopes: std::collections::HashMap::new(),
            dynamic_import_targets: std::collections::HashMap::new(),
            dynamic_import_namespace_modules: std::collections::HashSet::new(),
            dynamic_import_namespace_objects: std::collections::HashMap::new(),
            dynamic_import_specifier_map: std::collections::HashMap::new(),
            module_export_names: std::collections::HashMap::new(),
            re_export_map: std::collections::HashMap::new(),
            static_namespace_import_objects: std::collections::HashMap::new(),
            static_namespace_import_sources: Vec::new(),

            is_async_fn: false,
            is_async_generator_fn: false,
            is_generator_fn: false,
            async_state_counter: 0,
            captured_var_slots: std::collections::HashMap::new(),
            async_next_continuation_slot: 0,
            async_resume_blocks: Vec::new(),
            async_promise_scope_id: 0,
            async_dispatch_block: None,
            async_main_body_entry: None,
            async_main_param_ir_names: Vec::new(),
            async_env_scope_id: 0,
            async_state_scope_id: 0,
            async_resume_val_scope_id: 0,
            async_is_rejected_scope_id: 0,
            async_generator_scope_id: 0,
            async_closure_env_ir_name: None,
            pending_suspends: Vec::new(),
            strict_mode: false,
            is_arrow: false,
            is_method: false,
            arguments_param_count: 0,
            script_mode: false,
            diagnostic_source: None,
            diagnostic_filename: "input".into(),
            eval_mode: false,
            eval_has_scope_bridge: false,
            eval_var_writes_to_scope: false,
            eval_scope_record: false,
            eval_caller_has_arguments: false,
            active_using_vars: Vec::new(),
            array_bindings: std::collections::HashSet::new(),
            typedarray_bindings: std::collections::HashSet::new(),
            sab_bindings: std::collections::HashSet::new(),
            dataview_bindings: std::collections::HashSet::new(),
            eval_continue_block: None,
            new_expr_continue_block: None,
            await_continue_block: None,
            expr_merge_block: None,
            eval_completion: None,
        }
    }

    pub(crate) fn capture_async_context(&self) -> AsyncContextState {
        AsyncContextState {
            is_async_fn: self.is_async_fn,
            is_async_generator_fn: self.is_async_generator_fn,
            is_generator_fn: self.is_generator_fn,
            async_state_counter: self.async_state_counter,
            captured_var_slots: self.captured_var_slots.clone(),
            async_next_continuation_slot: self.async_next_continuation_slot,
            async_resume_blocks: self.async_resume_blocks.clone(),
            async_promise_scope_id: self.async_promise_scope_id,
            async_dispatch_block: self.async_dispatch_block,
            async_env_scope_id: self.async_env_scope_id,
            async_state_scope_id: self.async_state_scope_id,
            async_resume_val_scope_id: self.async_resume_val_scope_id,
            async_is_rejected_scope_id: self.async_is_rejected_scope_id,
            async_generator_scope_id: self.async_generator_scope_id,
            async_closure_env_ir_name: self.async_closure_env_ir_name.clone(),
            pending_suspends: self.pending_suspends.clone(),
        }
    }

    pub(crate) fn restore_async_context(&mut self, context: AsyncContextState) {
        self.is_async_fn = context.is_async_fn;
        self.is_async_generator_fn = context.is_async_generator_fn;
        self.is_generator_fn = context.is_generator_fn;
        self.async_state_counter = context.async_state_counter;
        self.captured_var_slots = context.captured_var_slots;
        self.async_next_continuation_slot = context.async_next_continuation_slot;
        self.async_resume_blocks = context.async_resume_blocks;
        self.async_promise_scope_id = context.async_promise_scope_id;
        self.async_dispatch_block = context.async_dispatch_block;
        self.async_env_scope_id = context.async_env_scope_id;
        self.async_state_scope_id = context.async_state_scope_id;
        self.async_resume_val_scope_id = context.async_resume_val_scope_id;
        self.async_is_rejected_scope_id = context.async_is_rejected_scope_id;
        self.async_generator_scope_id = context.async_generator_scope_id;
        self.async_closure_env_ir_name = context.async_closure_env_ir_name;
        self.pending_suspends = context.pending_suspends;
    }

    pub(crate) fn reset_async_context(&mut self) {
        self.restore_async_context(AsyncContextState {
            is_async_fn: false,
            is_async_generator_fn: false,
            is_generator_fn: false,
            async_state_counter: 0,
            captured_var_slots: std::collections::HashMap::new(),
            async_next_continuation_slot: 0,
            async_resume_blocks: Vec::new(),
            async_promise_scope_id: 0,
            async_dispatch_block: None,
            async_env_scope_id: 0,
            async_state_scope_id: 0,
            async_resume_val_scope_id: 0,
            async_is_rejected_scope_id: 0,
            async_generator_scope_id: 0,
            async_closure_env_ir_name: None,
            pending_suspends: Vec::new(),
        });
    }

    pub(crate) fn push_function_context(&mut self, name: impl Into<String>, entry: BasicBlockId) {
        self.async_context_stack.push(self.capture_async_context());
        self.function_stack.push(std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new(name, entry),
        ));
        // 压入函数作用域到现有作用域树，而非创建新树
        self.scopes.push_scope(ScopeKind::Function);
        // 记录当前函数的 scope id（用于逃逸分析）
        let fn_scope_id = self.scopes.current_scope_id();
        self.function_scope_id_stack.push(fn_scope_id);
        self.captured_names_stack.push(Vec::new());
        self.is_arrow_fn_stack.push(false); // 默认非箭头函数，箭头函数会单独设置
        self.function_lexical_home_object_stack
            .push(self.lexical_home_object);
        self.lexical_home_object = None;
        self.function_super_allowed_stack.push(self.super_allowed);
        self.function_super_call_allowed_stack
            .push(self.super_call_allowed);
        self.super_allowed = false;
        self.super_call_allowed = false;
        self.function_is_arrow_stack.push(self.is_arrow);
        self.function_is_method_stack.push(self.is_method);
        self.is_arrow = false;
        self.is_method = false;
        self.function_hoisted_stack.push((
            std::mem::take(&mut self.hoisted_vars),
            std::mem::take(&mut self.hoisted_vars_set),
        ));
        self.function_next_value_stack.push(self.next_value);
        self.function_next_temp_stack.push(self.next_temp);
        self.next_value = 0;
        self.next_temp = 0;
        self.function_try_contexts_stack
            .push(std::mem::take(&mut self.try_contexts));
        self.function_finally_stack_stack
            .push(std::mem::take(&mut self.finally_stack));
        self.function_label_stack_stack
            .push(std::mem::take(&mut self.label_stack));
        self.function_active_finalizers_stack
            .push(std::mem::take(&mut self.active_finalizers));
        self.function_pending_loop_label_stack
            .push(self.pending_loop_label.take());
        self.shared_env_stack.push(None);
        self.reset_async_context();
    }

    pub(crate) fn pop_function_context(&mut self) {
        self.current_function = self.function_stack.pop().expect("function stack underflow");
        // 弹出函数作用域，回到外层作用域
        self.scopes.pop_scope();
        self.function_scope_id_stack.pop();
        self.captured_names_stack.pop();
        self.is_arrow_fn_stack.pop();
        self.super_allowed = self
            .function_super_allowed_stack
            .pop()
            .expect("super context stack underflow");
        self.is_arrow = self
            .function_is_arrow_stack
            .pop()
            .expect("is_arrow stack underflow");
        self.is_method = self
            .function_is_method_stack
            .pop()
            .expect("is_method stack underflow");
        self.super_call_allowed = self
            .function_super_call_allowed_stack
            .pop()
            .expect("super call context stack underflow");
        self.lexical_home_object = self
            .function_lexical_home_object_stack
            .pop()
            .expect("lexical home object stack underflow");
        if self.shared_env_stack.len() > 1 {
            self.shared_env_stack.pop();
        }
        let (vars, set) = self
            .function_hoisted_stack
            .pop()
            .expect("hoisted stack underflow");
        self.hoisted_vars = vars;
        self.hoisted_vars_set = set;
        self.next_value = self
            .function_next_value_stack
            .pop()
            .expect("next value stack underflow");
        self.next_temp = self
            .function_next_temp_stack
            .pop()
            .expect("next temp stack underflow");
        self.try_contexts = self
            .function_try_contexts_stack
            .pop()
            .expect("try contexts stack underflow");
        self.finally_stack = self
            .function_finally_stack_stack
            .pop()
            .expect("finally stack stack underflow");
        self.label_stack = self
            .function_label_stack_stack
            .pop()
            .expect("label stack stack underflow");
        self.active_finalizers = self
            .function_active_finalizers_stack
            .pop()
            .expect("active finalizers stack underflow");
        self.pending_loop_label = self
            .function_pending_loop_label_stack
            .pop()
            .expect("pending loop label stack underflow");
        let async_context = self
            .async_context_stack
            .pop()
            .expect("async context stack underflow");
        self.restore_async_context(async_context);
    }

    pub(crate) fn current_function_scope_id(&self) -> usize {
        self.function_scope_id_stack.last().copied().unwrap_or(0)
    }

    pub(crate) fn binding_owner_function_scope(&self, binding: &CapturedBinding) -> usize {
        binding
            .scope_id
            .map(|scope_id| self.scopes.function_scope_for_scope(scope_id))
            .unwrap_or_else(|| self.current_function_scope_id())
    }

    pub(crate) fn binding_belongs_to_current_function(&self, binding: &CapturedBinding) -> bool {
        self.binding_owner_function_scope(binding) == self.current_function_scope_id()
    }

    pub(crate) fn record_capture(&mut self, binding: CapturedBinding) {
        if let Some(captured) = self.captured_names_stack.last_mut()
            && !captured.contains(&binding)
        {
            captured.push(binding);
        }
    }

    /// 构造器 ID 在类体 lowering 完成前未知时使用的占位符（随后由 patch 替换）。
    pub(crate) const PENDING_CTOR_FUNCTION_ID: FunctionId = FunctionId(u32::MAX);

    pub(crate) fn set_lexical_home_object_for_enclosing_method(
        &mut self,
        method_function_id: FunctionId,
        is_static: bool,
    ) {
        self.lexical_home_object = Some(if is_static {
            HomeObject::Constructor(method_function_id)
        } else {
            HomeObject::Prototype(method_function_id)
        });
    }

    pub(crate) fn patch_pending_ctor_home_object_references(
        &mut self,
        ctor_function_id: FunctionId,
    ) {
        let pending = Self::PENDING_CTOR_FUNCTION_ID;
        let count = self.module.functions().len();
        for idx in 0..count {
            let id = FunctionId(idx as u32);
            let Some(function) = self.module.function_mut(id) else {
                continue;
            };
            let Some(home) = function.home_object else {
                continue;
            };
            function.home_object = Some(match home {
                HomeObject::Prototype(id) if id == pending => {
                    HomeObject::Prototype(ctor_function_id)
                }
                HomeObject::Constructor(id) if id == pending => {
                    HomeObject::Constructor(ctor_function_id)
                }
                other => other,
            });
        }
    }

    pub(crate) fn captured_display_names(captured: &[CapturedBinding]) -> Vec<String> {
        captured.iter().map(CapturedBinding::display_name).collect()
    }

    pub(crate) fn is_shared_binding(&self, binding: &CapturedBinding) -> bool {
        self.shared_env_stack
            .last()
            .and_then(|shared| shared.as_ref())
            .is_some_and(|(_, names)| names.contains(binding))
    }

    pub(crate) fn shared_env_value(&self) -> Option<ValueId> {
        self.shared_env_stack
            .last()
            .and_then(|shared| shared.as_ref().map(|(value, _)| *value))
    }

    pub(crate) fn shared_env_ir_name(&self) -> String {
        format!("${}.$shared_env", self.current_function_scope_id())
    }

    /// 在函数 entry block (bb0) 将 `$shared_env` 初始化为 undefined。
    /// 非 async 函数的闭包捕获 fallback 依赖此 init（WASM local 未 init 为 0，非 undefined）。
    /// async 函数不在 bb0 init —— 其 `$shared_env` 由 continuation slot save/restore 跨 suspend 维持，
    /// bb0 init 会在每次 resume 时覆盖 restore 的值（A14）。
    pub(crate) fn initialize_shared_env_slot(&mut self) {
        if self.is_async_fn || self.is_async_generator_fn {
            return;
        }
        self.initialize_shared_env_slot_at(BasicBlockId(0));
    }

    /// 在指定 block 将 `$shared_env` 初始化为 undefined（不检查 async 状态）。
    /// 供 async 函数在 dispatch block 开头调用。
    pub(crate) fn initialize_shared_env_slot_at(&mut self, block: BasicBlockId) {
        let undef_const = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: undef_val,
                constant: undef_const,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::StoreVar {
                name: self.shared_env_ir_name(),
                value: undef_val,
            },
        );
    }

    pub(crate) fn append_env_key_const(
        &mut self,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> ValueId {
        let key_const = self
            .module
            .add_constant(Constant::String(binding.env_key()));
        let key_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: key_val,
                constant: key_const,
            },
        );
        key_val
    }

    pub(crate) fn load_captured_binding(
        &mut self,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> Result<ValueId, LoweringError> {
        let current_block = block;
        if self.binding_belongs_to_current_function(binding) {
            let shared_env = self.alloc_value();
            self.current_function.append_instruction(
                current_block,
                Instruction::LoadVar {
                    dest: shared_env,
                    name: self.shared_env_ir_name(),
                },
            );

            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                current_block,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );

            let env_missing = self.alloc_value();
            self.current_function.append_instruction(
                current_block,
                Instruction::Compare {
                    dest: env_missing,
                    op: CompareOp::StrictEq,
                    lhs: shared_env,
                    rhs: undef_val,
                },
            );

            let local_block = self.current_function.new_block();
            let env_block = self.current_function.new_block();
            let merge = self.current_function.new_block();
            self.current_function.set_terminator(
                current_block,
                Terminator::Branch {
                    condition: env_missing,
                    true_block: local_block,
                    false_block: env_block,
                },
            );

            let local_val = self.alloc_value();
            self.current_function.append_instruction(
                local_block,
                Instruction::LoadVar {
                    dest: local_val,
                    name: binding.var_ir_name(),
                },
            );
            self.current_function
                .set_terminator(local_block, Terminator::Jump { target: merge });

            let key_val = self.append_env_key_const(env_block, binding);
            let env_val = self.alloc_value();
            self.current_function.append_instruction(
                env_block,
                Instruction::GetProp {
                    dest: env_val,
                    object: shared_env,
                    key: key_val,
                },
            );
            self.current_function
                .set_terminator(env_block, Terminator::Jump { target: merge });

            let result = self.alloc_value();
            self.current_function.append_instruction(
                merge,
                Instruction::Phi {
                    dest: result,
                    sources: vec![
                        PhiSource {
                            predecessor: local_block,
                            value: local_val,
                        },
                        PhiSource {
                            predecessor: env_block,
                            value: env_val,
                        },
                    ],
                },
            );
            self.expr_merge_block = Some(merge);
            return Ok(result);
        }

        self.record_capture(binding.clone());
        let env_val = self.load_env_object(current_block);
        let key_val = self.append_env_key_const(current_block, binding);
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            current_block,
            Instruction::GetProp {
                dest,
                object: env_val,
                key: key_val,
            },
        );
        Ok(dest)
    }

    pub(crate) fn detect_param_arguments(params: &[swc_ast::Param]) -> bool {
        params.iter().any(|p| {
            let mut names = Vec::new();
            Self::extract_pat_bindings(std::slice::from_ref(&p.pat), &mut names);
            names.iter().any(|n| n == "arguments")
        })
    }

    /// 函数体（`BlockStmt`）是否引用了隐式 `arguments`。见 [`ArgumentsRefScan`]。
    pub(crate) fn body_references_arguments(body: &swc_ast::BlockStmt) -> bool {
        let mut scan = ArgumentsRefScan::default();
        body.visit_with(&mut scan);
        scan.found
    }

    /// 形参表（默认值 / 解构 / 计算属性等表达式）是否引用了隐式 `arguments`。
    /// 注意：形参**绑定名**本身若叫 `arguments` 也会被 `visit_ident` 命中——这无害，
    /// 因为那种情况已被 `current_function_has_param_arguments()` 守卫先行短路。
    pub(crate) fn params_reference_arguments(params: &[swc_ast::Param]) -> bool {
        let mut scan = ArgumentsRefScan::default();
        for p in params {
            p.visit_with(&mut scan);
        }
        scan.found
    }

    /// 综合判定：`swc_ast::Function` 形状的函数（形参 + 可选体）是否引用 `arguments`。
    /// 供 `emit_arguments_init` 的 `references_arguments` 实参使用。
    pub(crate) fn function_references_arguments(
        params: &[swc_ast::Param],
        body: Option<&swc_ast::BlockStmt>,
    ) -> bool {
        Self::params_reference_arguments(params)
            || body.is_some_and(Self::body_references_arguments)
    }

    /// 该 `swc_ast::Function` 是否需要物化 `arguments` 对象。
    ///
    /// = `is_async || is_generator || function_references_arguments(...)`。
    ///
    /// **为何对 async / generator 一律保留**：async 函数体编译为单个 `main$async` wasm 函数 +
    /// resume 状态机，其跨 `suspend` 的 save/restore 由后向 liveness + relooper 生成，对 IR 的
    /// 块/值布局极其敏感（见 wjsm async 异常通道笔记）。消除 arguments 物化会改变布局并使状态机
    /// 误编译。而 async / generator 必然 emit `new_promise` / `continuation.create`（直接 may-GC），
    /// 永远不可能是 no-GC——对它们消除 arguments 对 Layer 3 call-spill 省略**零收益**。故安全且无损地
    /// 将本优化限定在普通（非 async / 非 generator）函数。
    pub(crate) fn function_needs_arguments_object(func: &swc_ast::Function) -> bool {
        func.is_async
            || func.is_generator
            || Self::function_references_arguments(&func.params, func.body.as_ref())
    }

    /// 构造函数（`ParamOrTsParamProp` 形参 + 可选体）是否引用 `arguments`。
    pub(crate) fn ctor_references_arguments(ctor: &swc_ast::Constructor) -> bool {
        let mut scan = ArgumentsRefScan::default();
        for p in &ctor.params {
            p.visit_with(&mut scan);
        }
        if let Some(body) = &ctor.body {
            body.visit_with(&mut scan);
        }
        scan.found
    }

    pub(crate) fn count_regular_params(params: &[swc_ast::Param]) -> u32 {
        let mut n = 0u32;
        for p in params {
            if matches!(p.pat, swc_ast::Pat::Rest(_)) {
                break;
            }
            n += 1;
        }
        n
    }

    pub(crate) fn lower_module(
        mut self,
        module: &swc_ast::Module,
    ) -> Result<Program, LoweringError> {
        // 早错误：私有名词法有效性（AllPrivateIdentifiersValid）+ 类体私有名重复校验。
        // 必须先于降级执行；覆盖单模块、eval（均经此方法）路径。
        crate::lowerer_classes_ts::validate_private_names(module)?;
        // main 函数也需要 shared_env_stack 条目（顶层闭包需要在 main 中创建 env 对象）
        self.shared_env_stack.push(None);
        self.strict_mode = module_has_use_strict_directive(module);
        // Pre-scan: hoist variable declarations so let/const are in TDZ.
        self.predeclare_stmts(&module.body)?;

        let has_tla = has_top_level_await(module);
        let entry = if has_tla {
            self.init_async_main_context(module.span)?
        } else {
            BasicBlockId(0)
        };
        self.emit_hoisted_var_initializers(entry);

        // 初始化全局内置变量：undefined, NaN, Infinity
        // undefined
        let undef_const = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: undef_val,
                constant: undef_const,
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: "$0.undefined".to_string(),
                value: undef_val,
            },
        );
        // NaN
        let nan_const = self.module.add_constant(Constant::Number(f64::NAN));
        let nan_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: nan_val,
                constant: nan_const,
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: "$0.NaN".to_string(),
                value: nan_val,
            },
        );
        // Infinity
        let inf_const = self.module.add_constant(Constant::Number(f64::INFINITY));
        let inf_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: inf_val,
                constant: inf_const,
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: "$0.Infinity".to_string(),
                value: inf_val,
            },
        );

        // Math constants
        let math_constants: [(&str, f64); 8] = [
            ("$0.Math.E", std::f64::consts::E),
            ("$0.Math.LN10", std::f64::consts::LN_10),
            ("$0.Math.LN2", std::f64::consts::LN_2),
            ("$0.Math.LOG10E", std::f64::consts::LOG10_E),
            ("$0.Math.LOG2E", std::f64::consts::LOG2_E),
            ("$0.Math.PI", std::f64::consts::PI),
            ("$0.Math.SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
            ("$0.Math.SQRT2", std::f64::consts::SQRT_2),
        ];
        for (name, val) in math_constants {
            let c = self.module.add_constant(Constant::Number(val));
            let v = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::Const {
                    dest: v,
                    constant: c,
                },
            );
            self.current_function.append_instruction(
                entry,
                Instruction::StoreVar {
                    name: name.to_string(),
                    value: v,
                },
            );
        }

        // Number constants
        let number_constants: [(&str, f64); 8] = [
            ("$0.Number.EPSILON", f64::EPSILON),
            ("$0.Number.MAX_VALUE", f64::MAX),
            ("$0.Number.MIN_VALUE", f64::MIN_POSITIVE),
            ("$0.Number.MAX_SAFE_INTEGER", (1i64 << 53) as f64 - 1.0),
            ("$0.Number.MIN_SAFE_INTEGER", -((1i64 << 53) as f64 - 1.0)),
            ("$0.Number.NaN", f64::NAN),
            ("$0.Number.NEGATIVE_INFINITY", f64::NEG_INFINITY),
            ("$0.Number.POSITIVE_INFINITY", f64::INFINITY),
        ];
        for (name, val) in number_constants {
            let c = self.module.add_constant(Constant::Number(val));
            let v = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::Const {
                    dest: v,
                    constant: c,
                },
            );
            self.current_function.append_instruction(
                entry,
                Instruction::StoreVar {
                    name: name.to_string(),
                    value: v,
                },
            );
        }

        // 创建全局对象，用于两种模式下的 builtin 解析和 globalThis
        let global_obj = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(global_obj),
                builtin: Builtin::CreateGlobalObject,
                args: vec![],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: "$0.$global".to_string(),
                value: global_obj,
            },
        );

        // 设置 $this：script 模式 = 全局对象，module 模式 = undefined
        let this_val = if self.script_mode {
            global_obj
        } else {
            let undef_const = self.module.add_constant(Constant::Undefined);
            let v = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::Const {
                    dest: v,
                    constant: undef_const,
                },
            );
            v
        };
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: "$this".to_string(),
                value: this_val,
            },
        );

        let mut flow = StmtFlow::Open(entry);

        for item in &module.body {
            // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
            if matches!(flow, StmtFlow::Terminated) {
                continue;
            }
            match item {
                swc_ast::ModuleItem::Stmt(stmt) => {
                    flow = self.lower_stmt(stmt, flow)?;
                }
                swc_ast::ModuleItem::ModuleDecl(decl) => {
                    match decl {
                        // export const/let/var/function/class → 将内层声明作为普通语句处理
                        swc_ast::ModuleDecl::ExportDecl(export_decl) => {
                            flow = self
                                .lower_stmt(&swc_ast::Stmt::Decl(export_decl.decl.clone()), flow)?;
                        }
                        // export default expr → 将表达式作为普通语句处理
                        swc_ast::ModuleDecl::ExportDefaultExpr(default_expr) => {
                            let expr_stmt = swc_ast::ExprStmt {
                                span: default_expr.span,
                                expr: default_expr.expr.clone(),
                            };
                            flow = self.lower_expr_stmt(&expr_stmt, flow)?;
                        }
                        // export default function/class → 作为声明处理
                        swc_ast::ModuleDecl::ExportDefaultDecl(default_decl) => {
                            match &default_decl.decl {
                                swc_ast::DefaultDecl::Fn(fn_expr) => {
                                    if let Some(ident) = &fn_expr.ident {
                                        // export default function foo() {} → 作为命名函数声明处理
                                        let decl = swc_ast::Decl::Fn(swc_ast::FnDecl {
                                            ident: ident.clone(),
                                            declare: false,
                                            function: fn_expr.function.clone(),
                                        });
                                        flow = self.lower_stmt(&swc_ast::Stmt::Decl(decl), flow)?;
                                    } else {
                                        // 匿名默认导出函数 — 作为表达式语句求值
                                        let expr_stmt = swc_ast::ExprStmt {
                                            span: default_decl.span,
                                            expr: Box::new(swc_ast::Expr::Fn(fn_expr.clone())),
                                        };
                                        flow = self.lower_expr_stmt(&expr_stmt, flow)?;
                                    }
                                }
                                swc_ast::DefaultDecl::Class(class_expr) => {
                                    if let Some(ident) = &class_expr.ident {
                                        // export default class Foo {} → 作为命名类声明处理
                                        let decl = swc_ast::Decl::Class(swc_ast::ClassDecl {
                                            ident: ident.clone(),
                                            declare: false,
                                            class: class_expr.class.clone(),
                                        });
                                        flow = self.lower_stmt(&swc_ast::Stmt::Decl(decl), flow)?;
                                    }
                                    // 匿名默认导出类 — 跳过（无法作为表达式求值）
                                }
                                swc_ast::DefaultDecl::TsInterfaceDecl(_) => {
                                    // TypeScript 接口声明，跳过
                                }
                            }
                        }
                        // import 声明 → 单模块模式下跳过
                        swc_ast::ModuleDecl::Import(_) => {
                            // 单模块模式，跳过 import
                        }
                        // export * from / export { ... } → 暂时跳过
                        _ => {
                            // 暂不处理 re-exports
                        }
                    }
                }
            }
        }

        // If the last block is still open and hasn't been terminated, finalize it.
        match flow {
            StmtFlow::Open(block) => {
                if has_tla {
                    // TLA：resolve promise 然后 return
                    let undef_const = self.module.add_constant(Constant::Undefined);
                    let undef_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: undef_val,
                            constant: undef_const,
                        },
                    );
                    let promise_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::LoadVar {
                            dest: promise_val,
                            name: format!("${}.$promise", self.async_promise_scope_id),
                        },
                    );
                    self.current_function.append_instruction(
                        block,
                        Instruction::PromiseResolve {
                            promise: promise_val,
                            value: undef_val,
                        },
                    );
                    self.current_function
                        .set_terminator(block, Terminator::Return { value: None });
                } else {
                    // 非 TLA：检查 unreachable 并设置 Return
                    let is_unreachable = self
                        .current_function
                        .block(block)
                        .is_some_and(|b| matches!(b.terminator(), Terminator::Unreachable));
                    if self.eval_mode {
                        let return_value = if let Some(value) = self.eval_completion {
                            value
                        } else {
                            let undef_const = self.module.add_constant(Constant::Undefined);
                            let undef_val = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::Const {
                                    dest: undef_val,
                                    constant: undef_const,
                                },
                            );
                            undef_val
                        };
                        self.current_function.set_terminator(
                            block,
                            Terminator::Return {
                                value: Some(return_value),
                            },
                        );
                    } else if is_unreachable {
                        self.current_function
                            .set_terminator(block, Terminator::Return { value: None });
                    }
                }
            }
            StmtFlow::Terminated => {}
        }

        if has_tla {
            self.finalize_async_main()?;
        } else {
            let has_eval = self.current_function.has_eval();
            let blocks = self.current_function.into_blocks();
            let mut function = Function::new(MODULE_ENTRY_IR_NAME, BasicBlockId(0));
            function.set_has_eval(has_eval);
            if self.eval_mode {
                function.set_params(vec![EVAL_SCOPE_ENV_PARAM.to_string()]);
            }
            for block in blocks {
                function.push_block(block);
            }
            self.module.push_function(function);
        }
        Ok(self.module)
    }
}

impl Lowerer {
    /// 检查指定 Ident 是否为已知的 Array 绑定。
    pub(crate) fn is_array_binding(&self, ident: &swc_ast::Ident) -> bool {
        let name = ident.sym.to_string();
        if let Ok((scope_id, _)) = self.scopes.lookup(&name) {
            self.array_bindings.contains(&(scope_id, name))
        } else {
            false
        }
    }

    /// 检查指定 Ident 是否为已知的 TypedArray 绑定。
    pub(crate) fn is_typedarray_binding(&self, ident: &swc_ast::Ident) -> bool {
        let name = ident.sym.to_string();
        if let Ok((scope_id, _)) = self.scopes.lookup(&name) {
            self.typedarray_bindings.contains(&(scope_id, name))
        } else {
            false
        }
    }
    /// 检查指定 Ident 是否为已知的 SharedArrayBuffer 绑定。
    /// 仅对 `let sab = new SharedArrayBuffer(n)` 等静态已知绑定返回 true，
    /// 使 sab.slice / sab.grow 等在 lower_call_expr 走 CallBuiltin 优化路径；
    /// 动态 receiver 回退通用 Call，避免劫持 String/Array 的同名方法。
    pub(crate) fn is_sharedarraybuffer_binding(&self, ident: &swc_ast::Ident) -> bool {
        let name = ident.sym.to_string();
        if let Ok((scope_id, _)) = self.scopes.lookup(&name) {
            self.sab_bindings.contains(&(scope_id, name))
        } else {
            false
        }
    }
    /// 检查指定 Ident 是否为已知的 DataView 绑定。
    pub(crate) fn is_dataview_binding(&self, ident: &swc_ast::Ident) -> bool {
        let name = ident.sym.to_string();
        if let Ok((scope_id, _)) = self.scopes.lookup(&name) {
            self.dataview_bindings.contains(&(scope_id, name))
        } else {
            false
        }
    }
}
