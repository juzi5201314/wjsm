use super::*;

impl Lowerer {
    pub(crate) fn lower_async_gen_fn_decl(
        &mut self,
        fn_decl: &swc_ast::FnDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let (wrapper_fn_id, captured) =
            self.lower_async_gen_function(fn_decl, MethodSuperBinding::None)?;
        self.store_wrapper_in_outer_scope(
            flow,
            fn_decl.ident.sym.as_ref(),
            wrapper_fn_id,
            &captured,
            fn_decl.span(),
        )
    }

    /// 构建 async generator 的 body + wrapper 两个 IR 函数，返回 wrapper 的
    /// FunctionId 与捕获集合；由声明 / 表达式 / 方法路径共享。
    /// `method_super` 描述方法形态下 body 内 super 的绑定来源（见类型注释）。
    pub(crate) fn lower_async_gen_function(
        &mut self,
        fn_decl: &swc_ast::FnDecl,
        method_super: MethodSuperBinding,
    ) -> Result<(FunctionId, Vec<CapturedBinding>), LoweringError> {
        let name = fn_decl.ident.sym.to_string();
        let async_gen_name = format!("{name}$asyncgen");

        self.push_function_context(&async_gen_name, BasicBlockId(0));
        self.apply_function_strictness(fn_decl.function.body.as_ref());
        self.is_async_fn = true;
        self.is_async_generator_fn = true;
        self.async_state_counter = 1;
        self.captured_var_slots.clear();
        self.async_resume_blocks.clear();
        self.apply_method_super_binding(method_super);

        let (
            env_scope_id,
            this_scope_id,
            state_scope_id,
            resume_val_scope_id,
            is_rejected_scope_id,
            promise_scope_id,
            closure_env_scope_id,
        ) = self.declare_async_continuation_scopes(fn_decl.span())?;
        let gen_scope_id = self
            .scopes
            .declare("$generator", VarKind::Let, true)
            .map_err(|msg| self.error(fn_decl.span(), msg))?;

        self.async_env_scope_id = env_scope_id;
        self.async_state_scope_id = state_scope_id;
        self.async_resume_val_scope_id = resume_val_scope_id;
        self.async_is_rejected_scope_id = is_rejected_scope_id;
        self.async_promise_scope_id = promise_scope_id;
        self.async_generator_scope_id = gen_scope_id;
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

        // slot 2: generator
        let slot2_const = self.module.add_constant(Constant::Number(2.0));
        let slot2_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: slot2_val,
                constant: slot2_const,
            },
        );
        let gen_from_cont = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(gen_from_cont),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, slot2_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${gen_scope_id}.$generator"),
                value: gen_from_cont,
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

        self.set_arguments_params(&fn_decl.function.params);
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

        let body_entry = self.emit_pending_arguments_param_map(body_entry)?;
        let mut inner_flow = StmtFlow::Open(body_entry);
        if let Some(body) = &fn_decl.function.body {
            for stmt in &body.stmts {
                if matches!(inner_flow, StmtFlow::Terminated) {
                    continue;
                }
                inner_flow = self.lower_stmt(stmt, inner_flow)?;
            }
        }

        if let StmtFlow::Open(b) = inner_flow {
            let gen_val2 = self.alloc_value();
            self.current_function.append_instruction(
                b,
                Instruction::LoadVar {
                    dest: gen_val2,
                    name: format!("${gen_scope_id}.$generator"),
                },
            );
            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                b,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );
            self.current_function.append_instruction(
                b,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::AsyncGeneratorReturn,
                    args: vec![gen_val2, undef_val],
                },
            );
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }
        // ── 推迟的 save/restore：运行 liveness 分析并插入 save/restore ──
        self.resolve_pending_suspends();
        let continuation_slot_count = self.async_next_continuation_slot;
        self.emit_async_dispatch_switch(state_scope_id, dispatch_block, body_entry);

        let mut old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let known_callees = old_fn.take_known_callee_vars();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&async_gen_name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        if let Some(span) = self.span_to_source_span(fn_decl.span()) {
            ir_function.set_source_span(span);
        }
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        // 类方法：body 经续体 resume 调用，activation home 只能来自函数元数据。
        if let MethodSuperBinding::Static(home) = method_super {
            ir_function.home_object = Some(home);
        }
        for (ir_name, fn_id) in known_callees {
            ir_function.record_known_callee(ir_name, fn_id);
        }
        for b in blocks {
            ir_function.push_block(b);
        }
        let async_gen_fn_id = self.module.push_function(ir_function);

        self.pop_function_context();

        // ── 构建 wrapper 函数 ──
        self.push_function_context(&name, BasicBlockId(0));
        // wrapper 即方法本体：形参默认值等 wrapper 侧代码与 body 同严格性，
        // super 同样合法。
        self.apply_function_strictness(fn_decl.function.body.as_ref());
        self.apply_method_super_binding(method_super);

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
        let wrapper_entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(wrapper_entry);
        let wrapper_after_inits = self.emit_param_inits(
            &fn_decl.function.params,
            &wrapper_user_param_ir_names,
            wrapper_entry,
        )?;

        self.set_arguments_params(&fn_decl.function.params);
        let wrapper_after_inits = self.emit_arguments_init(
            wrapper_after_inits,
            Self::function_needs_arguments_object(&fn_decl.function),
        )?;
        self.eval_caller_has_arguments = Self::detect_param_arguments(&fn_decl.function.params)
            || self.scopes.lookup("arguments").is_ok();

        // ── wrapper 续体创建与启动 ──
        let func_ref_const = self
            .module
            .add_constant(Constant::FunctionRef(async_gen_fn_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );
        let (callee_val, env_val_opt) = if captured.is_empty() {
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

        let count_const = self
            .module
            .add_constant(Constant::Number(continuation_slot_count as f64));
        let count_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: count_val,
                constant: count_const,
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
        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: Some(cont_val),
                builtin: Builtin::ContinuationCreate,
                args: vec![callee_val, undef_val, count_val],
            },
        );
        // 对象字面量方法：把闭包 env 上的 home 转存为续体自有属性供 body 解析 super。
        if matches!(method_super, MethodSuperBinding::ClosureEnv) {
            self.emit_wrapper_home_transfer(wrapper_after_inits, cont_val);
        }

        // 启动异步生成器
        let gen_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: Some(gen_val),
                builtin: Builtin::AsyncGeneratorStart,
                args: vec![cont_val],
            },
        );

        // slot 2: 保存 generator
        let slot2_const = self.module.add_constant(Constant::Number(2.0));
        let slot2_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: slot2_val,
                constant: slot2_const,
            },
        );
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, slot2_val, gen_val],
            },
        );

        // slot 3: 保存 closure env
        let slot3_const = self.module.add_constant(Constant::Number(3.0));
        let slot3_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::Const {
                dest: slot3_val,
                constant: slot3_const,
            },
        );
        let env_for_slot = if let Some(env_val) = env_val_opt {
            env_val
        } else {
            undef_val
        };
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ContinuationSaveVar,
                args: vec![cont_val, slot3_val, env_for_slot],
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

        self.current_function.set_terminator(
            wrapper_after_inits,
            Terminator::Return {
                value: Some(gen_val),
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
        // 声明形态的 JS 名即 ident；表达式/方法路径经 fake decl 复用本函数，
        // 返回后按各自的 SetFunctionName 语义覆盖。
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
        if let MethodSuperBinding::Static(home) = method_super {
            wrapper_ir_function.home_object = Some(home);
        }
        for b in blocks {
            wrapper_ir_function.push_block(b);
        }
        wrapper_ir_function.set_needs_prototype(true);
        let wrapper_fn_id = self.module.push_function(wrapper_ir_function);
        self.pop_function_context();

        Ok((wrapper_fn_id, captured))
    }
}
