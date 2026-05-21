use super::*;

impl Lowerer {
    /// Lower an anonymous function expression `function(...) { ... }`.
    /// Returns a ValueId for the FunctionRef constant.
    pub(crate) fn lower_fn_expr(
        &mut self,
        fn_expr: &swc_ast::FnExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if fn_expr.function.is_async {
            return self.lower_async_fn_expr(fn_expr, block);
        }
        let name = fn_expr.ident.as_ref().map_or_else(
            || format!("anon_{}", self.module.functions().len()),
            |ident| ident.sym.to_string(),
        );
        self.push_function_context(&name, BasicBlockId(0));

        // 声明 $env（闭包环境对象）
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;
        // Register $this so that this-keyword expressions resolve.
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;

        // Register the function's own name (named function expression) so it is accessible within the body.
        if let Some(ref ident) = fn_expr.ident {
            let _ = self
                .scopes
                .declare(ident.sym.as_ref(), VarKind::Let, true)
                .map_err(|msg| self.error(fn_expr.span(), msg))?;
        }

        let param_ir_names =
            self.build_param_ir_names(&fn_expr.function.params, env_scope_id, this_scope_id)?;

        // Predeclare hoisted vars in body.
        if let Some(body) = &fn_expr.function.body {
            self.predeclare_block_stmts(&body.stmts)?;
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let body_entry = self.emit_param_inits(&fn_expr.function.params, &param_ir_names, entry)?;

        // Detect if calling context has explicit arguments binding
        let has_explicit_arguments = self.scopes.lookup("arguments").is_ok();
        self.eval_caller_has_arguments = Self::detect_param_arguments(&fn_expr.function.params) || has_explicit_arguments;

        let body_entry = self.emit_arguments_init(body_entry)?;

        // Lower body.
        let mut inner_flow = StmtFlow::Open(body_entry);
        if let Some(body) = &fn_expr.function.body {
            for stmt in &body.stmts {
                // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
                if matches!(inner_flow, StmtFlow::Terminated) {
                    continue;
                }
                inner_flow = self.lower_stmt(stmt, inner_flow)?;
            }
        }

        // Implicit return undefined.
        if let StmtFlow::Open(b) = inner_flow {
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        // Finalize IR function and push to module.
        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            ir_function.push_block(b);
        }
        let function_id = self.module.push_function(ir_function);

        // Restore outer context.
        self.pop_function_context();

        // 在外层函数中 emit 函数引用（闭包或直接 FunctionRef）
        let func_ref_const = self.module.add_constant(Constant::FunctionRef(function_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );

        // 如果有捕获变量，使用共享 env 对象 + CreateClosure
        let callee_val = if captured.is_empty() {
            func_ref_val
        } else {
            let env_val = self.ensure_shared_env(block, &captured, fn_expr.span())?;
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            closure_val
        };

        Ok(callee_val)
    }

    pub(crate) fn lower_async_fn_expr(
        &mut self,
        fn_expr: &swc_ast::FnExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let name = fn_expr.ident.as_ref().map_or_else(
            || format!("anon_{}", self.module.functions().len()),
            |ident| ident.sym.to_string(),
        );
        let async_name = format!("{name}$async");

        self.push_function_context(&async_name, BasicBlockId(0));
        self.is_async_fn = true;
        self.async_state_counter = 1;
        self.captured_var_slots.clear();
        self.async_resume_blocks.clear();

        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;

        if let Some(ref ident) = fn_expr.ident {
            let _ = self
                .scopes
                .declare(ident.sym.as_ref(), VarKind::Let, true)
                .map_err(|msg| self.error(fn_expr.span(), msg))?;
        }

        let state_scope_id = self
            .scopes
            .declare("$state", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;
        let resume_val_scope_id = self
            .scopes
            .declare("$resume_val", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;
        let is_rejected_scope_id = self
            .scopes
            .declare("$is_rejected", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;
        let promise_scope_id = self
            .scopes
            .declare("$promise", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;
        let closure_env_scope_id = self
            .scopes
            .declare("$closure_env", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;

        self.async_env_scope_id = env_scope_id;
        self.async_state_scope_id = state_scope_id;
        self.async_resume_val_scope_id = resume_val_scope_id;
        self.async_is_rejected_scope_id = is_rejected_scope_id;
        self.async_promise_scope_id = promise_scope_id;
        self.async_closure_env_ir_name = Some(format!("${closure_env_scope_id}.$closure_env"));

        let user_param_ir_names =
            self.build_param_ir_names(&fn_expr.function.params, env_scope_id, this_scope_id)?;
        self.init_async_continuation_slots(&user_param_ir_names, 4);

        let param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        if let Some(body) = &fn_expr.function.body {
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

        for (i, _param) in fn_expr.function.params.iter().enumerate() {
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
            self.emit_param_inits(&fn_expr.function.params, &user_param_ir_names, entry)?;

        // Detect if calling context has explicit arguments binding
        let has_explicit_arguments = self.scopes.lookup("arguments").is_ok();
        self.eval_caller_has_arguments = Self::detect_param_arguments(&fn_expr.function.params) || has_explicit_arguments;
        let after_inits = self.emit_arguments_init(after_inits)?;

        let dispatch_block = self.current_function.new_block();
        let body_entry = self.current_function.new_block();
        self.async_dispatch_block = Some(dispatch_block);

        self.current_function.set_terminator(
            after_inits,
            Terminator::Jump {
                target: dispatch_block,
            },
        );
        self.current_function
            .set_terminator(dispatch_block, Terminator::Unreachable);

        let mut inner_flow = StmtFlow::Open(body_entry);
        if let Some(body) = &fn_expr.function.body {
            for stmt in &body.stmts {
                if matches!(inner_flow, StmtFlow::Terminated) {
                    continue;
                }
                inner_flow = self.lower_stmt(stmt, inner_flow)?;
            }
        }

        if let StmtFlow::Open(b) = inner_flow {
            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                b,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );
            let promise_val = self.alloc_value();
            self.current_function.append_instruction(
                b,
                Instruction::LoadVar {
                    dest: promise_val,
                    name: format!("${promise_scope_id}.$promise"),
                },
            );
            self.current_function.append_instruction(
                b,
                Instruction::PromiseResolve {
                    promise: promise_val,
                    value: undef_val,
                },
            );
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        let resume_blocks = std::mem::take(&mut self.async_resume_blocks);
        if !resume_blocks.is_empty() {
            let state_val = self.alloc_value();
            self.current_function.append_instruction(
                dispatch_block,
                Instruction::LoadVar {
                    dest: state_val,
                    name: format!("${state_scope_id}.$state"),
                },
            );
            let zero_const_id = self.module.add_constant(Constant::Number(0.0));
            let mut switch_cases: Vec<SwitchCaseTarget> = Vec::new();
            switch_cases.push(SwitchCaseTarget {
                constant: zero_const_id,
                target: body_entry,
            });
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

        let continuation_slot_count = self.async_next_continuation_slot;

        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&async_name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            ir_function.push_block(b);
        }
        let async_fn_id = self.module.push_function(ir_function);

        self.pop_function_context();

        self.push_function_context(&name, BasicBlockId(0));

        let wrapper_env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;
        let wrapper_this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(fn_expr.span(), msg))?;

        let wrapper_user_param_ir_names = self.build_param_ir_names(
            &fn_expr.function.params,
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
            &fn_expr.function.params,
            &wrapper_user_param_ir_names,
            wrapper_entry,
        )?;

        // Detect if calling context has explicit arguments binding
        let has_explicit_arguments = self.scopes.lookup("arguments").is_ok();
        self.eval_caller_has_arguments = Self::detect_param_arguments(&fn_expr.function.params) || has_explicit_arguments;
        let wrapper_after_inits = self.emit_arguments_init(wrapper_after_inits)?;

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

        for (i, _arg) in fn_expr.function.params.iter().enumerate() {
            let param_ir_name = &wrapper_user_param_ir_names[2 + i];
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
        wrapper_ir_function.set_params(wrapper_user_param_ir_names.clone());
        wrapper_ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            wrapper_ir_function.push_block(b);
        }
        let wrapper_fn_id = self.module.push_function(wrapper_ir_function);

        self.pop_function_context();

        let wrapper_ref_const = self
            .module
            .add_constant(Constant::FunctionRef(wrapper_fn_id));
        let wrapper_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: wrapper_ref_val,
                constant: wrapper_ref_const,
            },
        );

        let callee_val = if captured.is_empty() {
            wrapper_ref_val
        } else {
            let env_val = self.ensure_shared_env(block, &captured, fn_expr.span())?;
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![wrapper_ref_val, env_val],
                },
            );
            closure_val
        };

        Ok(callee_val)
    }
}
