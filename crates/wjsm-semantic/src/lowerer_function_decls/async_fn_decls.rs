use super::*;

impl Lowerer {
    pub(crate) fn lower_async_fn_decl(
        &mut self,
        fn_decl: &swc_ast::FnDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let name = fn_decl.ident.sym.to_string();
        let async_name = format!("{name}$async");

        self.push_function_context(&async_name, BasicBlockId(0));
        self.apply_function_strictness(fn_decl.function.body.as_ref());
        self.is_async_fn = true;
        self.async_state_counter = 1;
        self.captured_var_slots.clear();
        self.async_resume_blocks.clear();

        let (
            env_scope_id,
            this_scope_id,
            state_scope_id,
            resume_val_scope_id,
            is_rejected_scope_id,
            promise_scope_id,
            closure_env_scope_id,
        ) = self.declare_async_continuation_scopes(fn_decl.span())?;

        self.async_env_scope_id = env_scope_id;
        self.async_state_scope_id = state_scope_id;
        self.async_resume_val_scope_id = resume_val_scope_id;
        self.async_is_rejected_scope_id = is_rejected_scope_id;
        self.async_promise_scope_id = promise_scope_id;
        self.async_closure_env_ir_name = Some(format!("${closure_env_scope_id}.$closure_env"));

        let user_param_ir_names =
            self.build_param_ir_names(&fn_decl.function.params, env_scope_id, this_scope_id)?;
        self.init_async_continuation_slots(&user_param_ir_names, 4);
        // 形参槽之后预留三个固定槽位：wrapper 把物化好的 arguments 对象、收集好的
        // rest 实参数组与调用时的原始 this 存进来（$this 形参被 resume 值复用）。
        let args_object_slot = self.async_next_continuation_slot;
        let rest_args_slot = args_object_slot + 1;
        let this_slot = args_object_slot + 2;
        self.async_next_continuation_slot += 3;

        let param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        if let Some(body) = &fn_decl.function.body {
            self.predeclare_block_stmts(&body.stmts)?;
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        // ── 从续体加载槽位 ──
        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::LoadVar {
                dest: cont_val,
                name: format!("${env_scope_id}.$env"),
            },
        );

        // slot 0: state
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

        // slot 1: is_rejected
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

        // resume_val 从 this 加载
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

        // 从续体槽位恢复原始 this（$this 形参承载的是 resume 值）。
        self.emit_body_this_restore_from_slot(entry, cont_val, this_slot, this_scope_id);

        // slot 2: promise
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

        // slot 3: closure_env
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

        // slots 4+: 用户参数（rest 形参不占 ir_name 槽位，按 ir_names 迭代）
        for (i, param_ir_name) in user_param_ir_names.iter().skip(2).enumerate() {
            let slot_const = self.module.add_constant(Constant::Number((4 + i) as f64));
            let slot_val = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::Const {
                    dest: slot_val,
                    constant: slot_const,
                },
            );
            let param_from_cont = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::CallBuiltin {
                    dest: Some(param_from_cont),
                    builtin: Builtin::ContinuationLoadVar,
                    args: vec![cont_val, slot_val],
                },
            );
            self.current_function.append_instruction(
                entry,
                Instruction::StoreVar {
                    name: param_ir_name.clone(),
                    value: param_from_cont,
                },
            );
        }

        // 从续体槽位取出 wrapper 侧物化的 arguments 对象与 rest 实参数组。
        self.set_arguments_source_from_slot(
            &fn_decl.function.params,
            entry,
            cont_val,
            args_object_slot,
        );
        self.set_rest_args_source_from_slot(
            &fn_decl.function.params,
            entry,
            cont_val,
            rest_args_slot,
        );

        let after_inits =
            self.emit_param_inits(&fn_decl.function.params, &user_param_ir_names, entry)?;

        self.arguments_param_count = Self::count_regular_params(&fn_decl.function.params);
        self.stage_arguments_alias_meta(&fn_decl.function, &user_param_ir_names);
        let after_inits = self.emit_arguments_init(
            after_inits,
            Self::function_needs_arguments_object(&fn_decl.function),
        )?;
        self.eval_caller_has_arguments = Self::detect_param_arguments(&fn_decl.function.params)
            || self.scopes.lookup("arguments").is_ok();
        let dispatch_block = self.current_function.new_block();
        let body_entry = self.current_function.new_block();
        self.async_dispatch_block = Some(dispatch_block);

        // 在 dispatch block 开头初始化 $shared_env = undefined（见 async_main.rs 同名注释）。
        self.initialize_shared_env_slot_at(dispatch_block);

        self.current_function.set_terminator(
            after_inits,
            Terminator::Jump {
                target: dispatch_block,
            },
        );

        self.current_function
            .set_terminator(dispatch_block, Terminator::Unreachable);

        let mut inner_flow = StmtFlow::Open(body_entry);
        if let Some(body) = &fn_decl.function.body {
            for stmt in &body.stmts {
                if matches!(inner_flow, StmtFlow::Terminated) {
                    continue;
                }
                inner_flow = self.lower_stmt(stmt, inner_flow)?;
            }
        }

        if let StmtFlow::Open(block) = inner_flow {
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
                    name: format!("${promise_scope_id}.$promise"),
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
        }
        // ── 推迟的 save/restore：运行 liveness 分析并插入 save/restore ──
        self.resolve_pending_suspends();
        self.emit_async_dispatch_switch(state_scope_id, dispatch_block, body_entry);

        let continuation_slot_count = self.async_next_continuation_slot;

        let mut old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let known_callees = old_fn.take_known_callee_vars();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&async_name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        if let Some(span) = self.span_to_source_span(fn_decl.span()) {
            ir_function.set_source_span(span);
        }
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        for (ir_name, fn_id) in known_callees {
            ir_function.record_known_callee(ir_name, fn_id);
        }
        for b in blocks {
            ir_function.push_block(b);
        }
        let async_fn_id = self.module.push_function(ir_function);

        self.pop_function_context();

        // ── 构建 wrapper 函数 ──
        self.push_function_context(&name, BasicBlockId(0));
        // wrapper 侧的形参默认值等用户代码与 body 同严格性。
        self.apply_function_strictness(fn_decl.function.body.as_ref());

        let wrapper_env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;
        let wrapper_this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;

        let wrapper_user_param_ir_names = self.build_param_ir_names(
            &fn_decl.function.params,
            wrapper_env_scope_id,
            wrapper_this_scope_id,
        )?;

        let _wrapper_param_ir_names = [
            format!("${wrapper_env_scope_id}.$env"),
            format!("${wrapper_this_scope_id}.$this"),
        ];

        let wrapper_entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(wrapper_entry);

        let wrapper_after_inits = self.emit_param_inits(
            &fn_decl.function.params,
            &wrapper_user_param_ir_names,
            wrapper_entry,
        )?;

        self.arguments_param_count = Self::count_regular_params(&fn_decl.function.params);
        self.stage_arguments_alias_meta(&fn_decl.function, &wrapper_user_param_ir_names);
        let wrapper_after_inits = self.emit_arguments_init(
            wrapper_after_inits,
            Self::function_needs_arguments_object(&fn_decl.function),
        )?;
        self.eval_caller_has_arguments = Self::detect_param_arguments(&fn_decl.function.params)
            || self.scopes.lookup("arguments").is_ok();

        // ── wrapper 续体创建与启动 ──
        let promise_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::NewPromise { dest: promise_val },
        );

        let func_ref_const = self.module.add_constant(Constant::FunctionRef(async_fn_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );

        // eval 桥下即使无捕获也把 wrapper 的 $env 传给 body（链根接调用方记录）。
        let (callee_val, env_val_opt) = if captured.is_empty() && !self.eval_scope_bridge_active() {
            (func_ref_val, None)
        } else {
            let env_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::LoadVar {
                    dest: env_val,
                    name: "$env".to_string(),
                },
            );
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            (closure_val, Some(env_val))
        };

        let count_val_num = continuation_slot_count;
        let count_const = self
            .module
            .add_constant(Constant::Number(count_val_num as f64));
        let count_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: count_val,
                constant: count_const,
            },
        );

        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: Some(cont_val),
                builtin: Builtin::ContinuationCreate,
                args: vec![callee_val, promise_val, count_val],
            },
        );
        // slot 0: 初始状态为入口状态 0
        let initial_state_slot_const = self.module.add_constant(Constant::Number(0.0));
        let initial_state_slot = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: initial_state_slot,
                constant: initial_state_slot_const,
            },
        );
        let initial_state_value_const = self.module.add_constant(Constant::Number(0.0));
        let initial_state_value = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: initial_state_value,
                constant: initial_state_value_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, initial_state_slot, initial_state_value],
            },
        );

        // slot 1: 初始完成状态为 fulfilled
        let initial_rejected_slot_const = self.module.add_constant(Constant::Number(1.0));
        let initial_rejected_slot = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: initial_rejected_slot,
                constant: initial_rejected_slot_const,
            },
        );
        let initial_rejected_value_const = self.module.add_constant(Constant::Bool(false));
        let initial_rejected_value = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: initial_rejected_value,
                constant: initial_rejected_value_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, initial_rejected_slot, initial_rejected_value],
            },
        );

        // slot 2: 保存 promise
        let save_slot0_const = self.module.add_constant(Constant::Number(2.0));
        let save_slot0_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: save_slot0_val,
                constant: save_slot0_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, save_slot0_val, promise_val],
            },
        );

        // slot 3: 保存 closure env
        let save_slot1_const = self.module.add_constant(Constant::Number(3.0));
        let save_slot1_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: save_slot1_val,
                constant: save_slot1_const,
            },
        );
        let env_for_slot = if let Some(ev) = env_val_opt {
            ev
        } else {
            let ud_const = self.module.add_constant(Constant::Undefined);
            let ud_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::Const {
                    dest: ud_val,
                    constant: ud_const,
                },
            );
            ud_val
        };
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, save_slot1_val, env_for_slot],
            },
        );

        // slots 4+: 保存用户参数到续体槽位（rest 形参不占 ir_name 槽位，按 ir_names 迭代）
        for (i, param_ir_name) in wrapper_user_param_ir_names.iter().skip(2).enumerate() {
            let arg_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::LoadVar {
                    dest: arg_val,
                    name: param_ir_name.clone(),
                },
            );
            let save_slot_const = self.module.add_constant(Constant::Number((4 + i) as f64));
            let save_slot_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::Const {
                    dest: save_slot_val,
                    constant: save_slot_const,
                },
            );
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ContinuationSaveVar,
                    args: vec![cont_val, save_slot_val, arg_val],
                },
            );
        }

        // 把 wrapper 物化的 arguments 对象、收集的 rest 实参数组与原始 this 保存进固定槽位。
        self.emit_wrapper_arguments_slot_save(
            &fn_decl.function.params,
            wrapper_after_inits,
            cont_val,
            args_object_slot,
        );
        self.emit_wrapper_rest_args_slot_save(
            &fn_decl.function.params,
            wrapper_after_inits,
            cont_val,
            rest_args_slot,
        );
        self.emit_wrapper_this_slot_save(
            wrapper_after_inits,
            cont_val,
            this_slot,
            wrapper_this_scope_id,
        );

        // 启动异步执行
        let zero_const = self.module.add_constant(Constant::Number(0.0));
        let zero_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: zero_val,
                constant: zero_const,
            },
        );
        let undef_const = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: undef_val,
                constant: undef_const,
            },
        );
        let false_const = self.module.add_constant(Constant::Bool(false));
        let false_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: false_val,
                constant: false_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::AsyncFunctionResume,
                args: vec![callee_val, cont_val, zero_val, undef_val, false_val],
            },
        );

        self.current_function.set_terminator(
            wrapper_after_inits,
            Terminator::Return {
                value: Some(promise_val),
            },
        );

        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut wrapper_ir_function = Function::new(&name, BasicBlockId(0));
        wrapper_ir_function.set_has_eval(has_eval);
        wrapper_ir_function.set_js_name(&name);
        wrapper_ir_function.set_js_length(Self::expected_param_count(&fn_decl.function.params));
        if let Some(span) = self.span_to_source_span(fn_decl.span()) {
            wrapper_ir_function.set_source_span(span);
        }
        if let Some(text) = self.span_source_text(fn_decl.span()) {
            wrapper_ir_function.set_source_text(text);
        }
        wrapper_ir_function.set_params(wrapper_user_param_ir_names.clone());
        wrapper_ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            wrapper_ir_function.push_block(b);
        }
        wrapper_ir_function.set_needs_prototype(true);
        let wrapper_fn_id = self.module.push_function(wrapper_ir_function);

        self.pop_function_context();

        self.store_wrapper_in_outer_scope(flow, &name, wrapper_fn_id, &captured, fn_decl.span())
    }
}
