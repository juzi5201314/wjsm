use super::*;

impl Lowerer {
    pub(crate) fn lower_gen_fn_decl(
        &mut self,
        fn_decl: &swc_ast::FnDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let name = fn_decl.ident.sym.to_string();
        let gen_body_name = format!("{name}$gen");

        self.push_function_context(&gen_body_name, BasicBlockId(0));
        self.is_generator_fn = true;
        self.async_state_counter = 1;
        self.captured_var_slots.clear();
        self.async_resume_blocks.clear();

        let (
            env_scope_id,
            this_scope_id,
            state_scope_id,
            resume_val_scope_id,
            completion_scope_id,
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
        self.async_is_rejected_scope_id = completion_scope_id;
        self.async_promise_scope_id = promise_scope_id;
        self.async_generator_scope_id = gen_scope_id;
        self.async_closure_env_ir_name = Some(format!("${closure_env_scope_id}.$closure_env"));

        let user_param_ir_names =
            self.build_param_ir_names(&fn_decl.function.params, env_scope_id, this_scope_id)?;
        self.init_async_continuation_slots(&user_param_ir_names, 5);
        let param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        if let Some(body) = &fn_decl.function.body {
            self.predeclare_block_stmts(&body.stmts)?;
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);
        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::LoadVar {
                dest: cont_val,
                name: format!("${env_scope_id}.$env"),
            },
        );
        for (slot, name) in [
            (0usize, format!("${state_scope_id}.$state")),
            (1usize, format!("${completion_scope_id}.$is_rejected")),
            (2usize, format!("${gen_scope_id}.$generator")),
            (3usize, format!("${closure_env_scope_id}.$closure_env")),
        ] {
            let slot_const = self.module.add_constant(Constant::Number(slot as f64));
            let slot_val = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::Const {
                    dest: slot_val,
                    constant: slot_const,
                },
            );
            let loaded = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::CallBuiltin {
                    dest: Some(loaded),
                    builtin: Builtin::ContinuationLoadVar,
                    args: vec![cont_val, slot_val],
                },
            );
            self.current_function.append_instruction(
                entry,
                Instruction::StoreVar {
                    name,
                    value: loaded,
                },
            );
        }

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
        let this_slot_const = self.module.add_constant(Constant::Number(4.0));
        let this_slot_val = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::Const {
                dest: this_slot_val,
                constant: this_slot_const,
            },
        );
        let original_this = self.alloc_value();
        self.current_function.append_instruction(
            entry,
            Instruction::CallBuiltin {
                dest: Some(original_this),
                builtin: Builtin::ContinuationLoadVar,
                args: vec![cont_val, this_slot_val],
            },
        );
        self.current_function.append_instruction(
            entry,
            Instruction::StoreVar {
                name: format!("${this_scope_id}.$this"),
                value: original_this,
            },
        );

        for (i, _param) in fn_decl.function.params.iter().enumerate() {
            let slot_const = self.module.add_constant(Constant::Number((5 + i) as f64));
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
            let param_ir_name = &user_param_ir_names[2 + i];
            self.current_function.append_instruction(
                entry,
                Instruction::StoreVar {
                    name: param_ir_name.clone(),
                    value: param_from_cont,
                },
            );
        }

        let after_inits =
            self.emit_param_inits(&fn_decl.function.params, &user_param_ir_names, entry)?;
        self.arguments_param_count = Self::count_regular_params(&fn_decl.function.params);
        let after_inits = self.emit_arguments_init(
            after_inits,
            Self::function_needs_arguments_object(&fn_decl.function),
        )?;
        self.eval_caller_has_arguments = Self::detect_param_arguments(&fn_decl.function.params)
            || self.scopes.lookup("arguments").is_ok();

        let dispatch_block = self.current_function.new_block();
        let body_entry = self.current_function.new_block();
        self.async_dispatch_block = Some(dispatch_block);
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

        if let StmtFlow::Open(b) = inner_flow {
            let gen_val = self.alloc_value();
            self.current_function.append_instruction(
                b,
                Instruction::LoadVar {
                    dest: gen_val,
                    name: format!("${gen_scope_id}.$generator"),
                },
            );
            let undef_val = self.alloc_undefined_value(b);
            let result = self.alloc_value();
            self.current_function.append_instruction(
                b,
                Instruction::CallBuiltin {
                    dest: Some(result),
                    builtin: Builtin::GeneratorReturn,
                    args: vec![gen_val, undef_val],
                },
            );
            self.current_function.set_terminator(
                b,
                Terminator::Return {
                    value: Some(result),
                },
            );
        }

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
        let mut ir_function = Function::new(&gen_body_name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        for (ir_name, fn_id) in known_callees {
            ir_function.record_known_callee(ir_name, fn_id);
        }
        for b in blocks {
            ir_function.push_block(b);
        }
        let gen_body_fn_id = self.module.push_function(ir_function);
        self.pop_function_context();

        self.push_function_context(&name, BasicBlockId(0));
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
        self.arguments_param_count = Self::count_regular_params(&fn_decl.function.params);
        let wrapper_after_inits = self.emit_arguments_init(
            wrapper_after_inits,
            Self::function_needs_arguments_object(&fn_decl.function),
        )?;
        self.eval_caller_has_arguments = Self::detect_param_arguments(&fn_decl.function.params)
            || self.scopes.lookup("arguments").is_ok();

        let func_ref_const = self
            .module
            .add_constant(Constant::FunctionRef(gen_body_fn_id));
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
                    name: format!("${wrapper_env_scope_id}.$env"),
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
        let undef_val = self.alloc_undefined_value(wrapper_after_inits);
        let cont_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: Some(cont_val),
                builtin: Builtin::ContinuationCreate,
                args: vec![callee_val, undef_val, count_val],
            },
        );
        let gen_val = self.alloc_value();
        self.current_function.append_instruction(
            wrapper_after_inits,
            Instruction::CallBuiltin {
                dest: Some(gen_val),
                builtin: Builtin::GeneratorStart,
                args: vec![cont_val],
            },
        );

        for (slot, value) in [
            (2usize, gen_val),
            (3usize, env_val_opt.unwrap_or(undef_val)),
            {
                let this_val = self.alloc_value();
                self.current_function.append_instruction(
                    wrapper_after_inits,
                    Instruction::LoadVar {
                        dest: this_val,
                        name: format!("${wrapper_this_scope_id}.$this"),
                    },
                );
                (4usize, this_val)
            },
        ] {
            let slot_const = self.module.add_constant(Constant::Number(slot as f64));
            let slot_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::Const {
                    dest: slot_val,
                    constant: slot_const,
                },
            );
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ContinuationSaveVar,
                    args: vec![cont_val, slot_val, value],
                },
            );
        }

        for (i, _arg) in fn_decl.function.params.iter().enumerate() {
            let param_ir_name = &wrapper_user_param_ir_names[2 + i];
            let arg_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::LoadVar {
                    dest: arg_val,
                    name: param_ir_name.clone(),
                },
            );
            let slot_const = self.module.add_constant(Constant::Number((5 + i) as f64));
            let slot_val = self.alloc_value();
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::Const {
                    dest: slot_val,
                    constant: slot_const,
                },
            );
            self.current_function.append_instruction(
                wrapper_after_inits,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ContinuationSaveVar,
                    args: vec![cont_val, slot_val, arg_val],
                },
            );
        }

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
        wrapper_ir_function.set_params(wrapper_user_param_ir_names);
        wrapper_ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            wrapper_ir_function.push_block(b);
        }
        let wrapper_fn_id = self.module.push_function(wrapper_ir_function);
        self.pop_function_context();

        self.store_wrapper_in_outer_scope(flow, &name, wrapper_fn_id, &captured, fn_decl.span())
    }
}
