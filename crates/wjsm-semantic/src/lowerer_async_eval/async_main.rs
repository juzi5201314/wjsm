use super::*;

impl Lowerer {
    pub(crate) fn init_async_continuation_slots(
        &mut self,
        param_ir_names: &[String],
        first_param_slot: u32,
    ) {
        self.captured_var_slots.clear();
        for (offset, name) in param_ir_names.iter().skip(2).enumerate() {
            self.captured_var_slots
                .insert(name.clone(), first_param_slot + offset as u32);
        }
        self.async_next_continuation_slot =
            first_param_slot + param_ir_names.len().saturating_sub(2) as u32;
    }
    /// 为包含 top-level await 的模块设置 async main 上下文。
    /// 在 entry block (block 0) 中 emit 从 continuation 加载状态的指令，
    /// 创建 dispatch block 和 body entry block，返回 body_entry。
    /// 调用者应使用返回的 body_entry 作为后续 emit 的起始 block。
    pub(crate) fn init_async_main_context(
        &mut self,
        span: swc_core::common::Span,
    ) -> Result<BasicBlockId, LoweringError> {
        self.is_async_fn = true;
        self.async_state_counter = 1;
        self.captured_var_slots.clear();
        self.async_resume_blocks.clear();
        // 为 main 函数设置函数上下文栈（async_visible_binding_names 依赖此栈）
        let fn_scope_id = self.scopes.current_scope_id();
        self.function_scope_id_stack.push(fn_scope_id);
        self.captured_names_stack.push(Vec::new());
        self.is_arrow_fn_stack.push(false);

        let entry = BasicBlockId(0);

        // 声明 async 内部变量
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let state_scope_id = self
            .scopes
            .declare("$state", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let resume_val_scope_id = self
            .scopes
            .declare("$resume_val", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let is_rejected_scope_id = self
            .scopes
            .declare("$is_rejected", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let promise_scope_id = self
            .scopes
            .declare("$promise", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let closure_env_scope_id = self
            .scopes
            .declare("$closure_env", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;

        self.async_env_scope_id = env_scope_id;
        self.async_state_scope_id = state_scope_id;
        self.async_resume_val_scope_id = resume_val_scope_id;
        self.async_is_rejected_scope_id = is_rejected_scope_id;
        self.async_promise_scope_id = promise_scope_id;
        self.async_closure_env_ir_name = Some(format!("${closure_env_scope_id}.$closure_env"));

        // 无用户参数，continuation slots 从 4 开始
        self.init_async_continuation_slots(&[], 4);

        let param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];
        self.async_main_param_ir_names = param_ir_names;

        // ── entry block: 从 continuation 加载状态 ──

        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::LoadVar {
                dest: cont_val,
                name: format!("${env_scope_id}.$env"),
            },
        );

        // continuation slot 0 → $state
        let slot0_const = self.module.add_constant(Constant::Number(0.0));
        let slot0_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot0_val,
                constant: slot0_const,
            },
        );
        let state_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(state_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot0_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${state_scope_id}.$state"),
                value: state_from_cont,
            },
        );

        // continuation slot 1 → $is_rejected
        let slot1_const = self.module.add_constant(Constant::Number(1.0));
        let slot1_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot1_val,
                constant: slot1_const,
            },
        );
        let is_rejected_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(is_rejected_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot1_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${is_rejected_scope_id}.$is_rejected"),
                value: is_rejected_from_cont,
            },
        );

        // $this → $resume_val
        let resume_val_from_this = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::LoadVar {
                dest: resume_val_from_this,
                name: format!("${this_scope_id}.$this"),
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${resume_val_scope_id}.$resume_val"),
                value: resume_val_from_this,
            },
        );

        // continuation slot 2 → $promise
        let slot2_const = self.module.add_constant(Constant::Number(2.0));
        let slot2_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot2_val,
                constant: slot2_const,
            },
        );
        let promise_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(promise_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot2_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${promise_scope_id}.$promise"),
                value: promise_from_cont,
            },
        );

        // continuation slot 3 → $closure_env
        let slot3_const = self.module.add_constant(Constant::Number(3.0));
        let slot3_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot3_val,
                constant: slot3_const,
            },
        );
        let env_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(env_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot3_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${closure_env_scope_id}.$closure_env"),
                value: env_from_cont,
            },
        );

        // 创建 dispatch block 和 body entry
        let dispatch_block = self.current_function.new_block();
        let body_entry = self.current_function.new_block();
        self.async_dispatch_block = Some(dispatch_block);
        self.async_main_body_entry = Some(body_entry);

        // 在 dispatch block 开头将 $shared_env 初始化为 undefined。
        // async 函数不能像普通函数那样在 bb0 init（bb0 是 continuation load，每次 resume 重新执行，
        // 会覆盖 resume block 的 continuation restore）。dispatch block 只在首次执行和 resume 时到达：
        // 首次执行 → init undefined → body 中 ensure_shared_env 覆盖为 env object；
        // resume → init undefined → resume block 的 continuation restore 覆盖为 saved 值（若有）。
        self.initialize_shared_env_slot_at(dispatch_block);

        self.current_function.set_terminator(
            entry,
            Terminator::Jump {
                target: dispatch_block,
            },
        );
        self.current_function
            .set_terminator(dispatch_block, Terminator::Unreachable);

        Ok(body_entry)
    }

    /// 生成 dispatch block，保存 main$async 函数，创建 wrapper main 函数。
    /// 调用前需要确保：
    /// - 模块体的最后一个 block 已正确终止（open block 需要 emit PromiseResolve + Return）
    /// - async_resume_blocks 已填充
    pub(crate) fn finalize_async_main(&mut self) -> Result<(), LoweringError> {
        // ── 推迟的 save/restore：运行 liveness 分析并插入 save/restore ──
        self.resolve_pending_suspends();
        let dispatch_block = self
            .async_dispatch_block
            .expect("async_dispatch_block not set");
        let body_entry = self
            .async_main_body_entry
            .expect("async_main_body_entry not set");

        // ── 1. 生成 dispatch block（状态机 switch）──
        let resume_blocks = std::mem::take(&mut self.async_resume_blocks);
        if !resume_blocks.is_empty() {
            let state_val = self.alloc_value();
            self.current_function.append_instruction(
                dispatch_block,
                Instruction::LoadVar {
                    dest: state_val,
                    name: format!("${}.$state", self.async_state_scope_id),
                },
            );

            let zero_const_id = self.module.add_constant(Constant::Number(0.0));
            let mut switch_cases = vec![SwitchCaseTarget {
                constant: zero_const_id,
                target: body_entry,
            }];

            for (state_num, target_block) in &resume_blocks {
                let case_const_id = self
                    .module
                    .add_constant(Constant::Number(*state_num as f64));
                switch_cases.push(SwitchCaseTarget {
                    constant: case_const_id,
                    target: *target_block,
                });
            }

            let default_block = self.current_function.new_block();
            let exit_block = self.current_function.new_block();
            self.current_function
                .set_terminator(default_block, Terminator::Return { value: None });
            self.current_function
                .set_terminator(exit_block, Terminator::Unreachable);

            self.current_function.set_terminator(
                dispatch_block,
                Terminator::Switch {
                    value: state_val,
                    cases: switch_cases,
                    default_block,
                    exit_block,
                },
            );
        } else {
            self.current_function
                .set_terminator(dispatch_block, Terminator::Jump { target: body_entry });
        }

        // ── 2. 提取 main$async 函数 ──
        let continuation_slot_count = self.async_next_continuation_slot;
        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut async_fn = Function::new("main$async", BasicBlockId(0));
        async_fn.set_has_eval(has_eval);
        async_fn.set_params(self.async_main_param_ir_names.clone());
        for b in blocks {
            async_fn.push_block(b);
        }
        let async_fn_id = self.module.push_function(async_fn);

        // ── 3. 创建 wrapper main 函数 ──
        self.next_value = 0;
        self.next_temp = 0;

        let wrapper_entry = BasicBlockId(0);

        // NewPromise
        let promise_val = self.alloc_value();
        self.current_function
            .append_instruction(wrapper_entry, Instruction::NewPromise { dest: promise_val });

        // FunctionRef for main$async
        let func_ref_const = self.module.add_constant(Constant::FunctionRef(async_fn_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );

        // ContinuationCreate(func_ref, promise, slot_count)
        let count_const = self
            .module
            .add_constant(Constant::Number(continuation_slot_count as f64));
        let count_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: count_val,
                constant: count_const,
            },
        );
        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::CallBuiltin {
                dest: Some(cont_val),
                builtin: Builtin::ContinuationCreate,
                args: vec![func_ref_val, promise_val, count_val],
            },
        );

        // ContinuationSaveVar slot 2 = promise
        let save_slot2_const = self.module.add_constant(Constant::Number(2.0));
        let save_slot2_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: save_slot2_val,
                constant: save_slot2_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, save_slot2_val, promise_val],
            },
        );

        // ContinuationSaveVar slot 3 = undefined (no closure env)
        let save_slot3_const = self.module.add_constant(Constant::Number(3.0));
        let save_slot3_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: save_slot3_val,
                constant: save_slot3_const,
            },
        );
        let undef_const = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: undef_val,
                constant: undef_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, save_slot3_val, undef_val],
            },
        );

        // AsyncFunctionResume(func_ref, continuation, state=0, resume_val=undefined, is_rejected=false)
        let zero_const = self.module.add_constant(Constant::Number(0.0));
        let zero_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: zero_val,
                constant: zero_const,
            },
        );
        let false_const = self.module.add_constant(Constant::Bool(false));
        let false_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::Const {
                dest: false_val,
                constant: false_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_entry,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::AsyncFunctionResume,
                args: vec![func_ref_val, cont_val, zero_val, undef_val, false_val],
            },
        );

        self.current_function
            .set_terminator(wrapper_entry, Terminator::Return { value: None });

        // 提取 wrapper blocks，推入模块
        let wrapper_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let wrapper_has_eval = wrapper_fn.has_eval();
        let wrapper_blocks = wrapper_fn.into_blocks();
        let mut wrapper_ir = Function::new(MODULE_ENTRY_IR_NAME, BasicBlockId(0));
        wrapper_ir.set_has_eval(wrapper_has_eval);
        wrapper_ir.set_params(self.async_main_param_ir_names.clone());
        for b in wrapper_blocks {
            wrapper_ir.push_block(b);
        }
        self.module.push_function(wrapper_ir);

        Ok(())
    }

    pub(crate) fn is_async_internal_binding(name: &str) -> bool {
        let binding_name = if let Some((scope, binding)) = name.split_once('.') {
            if scope.len() > 1
                && scope.starts_with('$')
                && scope[1..].bytes().all(|byte| byte.is_ascii_digit())
            {
                binding
            } else {
                name
            }
        } else {
            name
        };

        matches!(
            binding_name,
            "$env"
                | "$this"
                | "$state"
                | "$resume_val"
                | "$is_rejected"
                | "$promise"
                | "$closure_env"
                | "$generator"
        ) || binding_name.starts_with("$tmp.")
    }
}
