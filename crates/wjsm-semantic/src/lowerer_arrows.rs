use super::*;

impl Lowerer {
    /// Lower an arrow function expression `(params) => expr` or `(params) => { ... }`.
    pub(crate) fn lower_arrow_expr(
        &mut self,
        arrow: &swc_ast::ArrowExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if arrow.is_async {
            return self.lower_async_arrow_expr(arrow, block);
        }
        let inherited_home_object = self.lexical_home_object;
        let outer_super_allowed = self.super_allowed;
        let outer_super_call_allowed = self.super_call_allowed;
        let name = format!("arrow_{}", self.module.functions().len());
        self.push_function_context(&name, BasicBlockId(0));
        // 标记当前为箭头函数；箭头函数继承外层 super 绑定。
        self.is_arrow = true;
        *self.is_arrow_fn_stack.last_mut().unwrap() = true;
        self.lexical_home_object = inherited_home_object;
        self.super_allowed = outer_super_allowed;
        self.super_call_allowed = outer_super_call_allowed;
        // 声明 $env（闭包环境对象）
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;
        // 箭头函数声明 $this 参数占位（WASM 调用约定需要），但内部 this 通过 env 捕获读取
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;

        let param_ir_names =
            self.build_arrow_param_ir_names(&arrow.params, env_scope_id, this_scope_id)?;

        let entry = BasicBlockId(0);
        let mut inner_flow;

        match arrow.body.as_ref() {
            swc_ast::BlockStmtOrExpr::BlockStmt(block_stmt) => {
                // Predeclare and lower block body.
                self.predeclare_block_stmts(&block_stmt.stmts)?;
                self.emit_hoisted_var_initializers(entry);
                let body_entry =
                    self.emit_arrow_param_inits(&arrow.params, &param_ir_names, entry)?;
                inner_flow = StmtFlow::Open(body_entry);
                for stmt in &block_stmt.stmts {
                    // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
                    if matches!(inner_flow, StmtFlow::Terminated) {
                        continue;
                    }
                    inner_flow = self.lower_stmt(stmt, inner_flow)?;
                }
            }
            swc_ast::BlockStmtOrExpr::Expr(expr) => {
                // Expression body: param inits, lower expr, then return it.
                self.emit_hoisted_var_initializers(entry);
                let body_entry =
                    self.emit_arrow_param_inits(&arrow.params, &param_ir_names, entry)?;
                let val = self.lower_expr(expr, body_entry)?;
                // 如果表达式 lowered 时产生了分支（如三元运算符），body_entry 已经被 set_terminator，
                // 此时需要在 merge block 处加 Return，而不是覆盖 body_entry 的 Branch。
                let has_branch = self
                    .current_function
                    .block(body_entry)
                    .map(|b| !matches!(b.terminator(), Terminator::Unreachable))
                    .unwrap_or(false);
                if has_branch {
                    // lower_cond 创建的 merge block 是最后一个 block，给它加 Return
                    let last_block = self.current_function.last_block_id();
                    self.current_function
                        .set_terminator(last_block, Terminator::Return { value: Some(val) });
                } else {
                    self.current_function
                        .set_terminator(body_entry, Terminator::Return { value: Some(val) });
                }
                self.expr_merge_block = None;
                inner_flow = StmtFlow::Terminated;
            }
        }

        // Implicit return undefined if no explicit return.
        if let StmtFlow::Open(b) = inner_flow {
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        // Finalize IR function.
        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        if let Some(span) = self.span_to_source_span(arrow.span()) {
            ir_function.set_source_span(span);
        }
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        if let Some(home) = inherited_home_object {
            ir_function.home_object = Some(home);
        }
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

        let callee_val = if captured.is_empty() {
            func_ref_val
        } else {
            let mut closure_block = block;
            let env_val = self.ensure_shared_env(closure_block, &captured, arrow.span)?;
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
            closure_val
        };

        Ok(callee_val)
    }

    pub(crate) fn lower_async_arrow_expr(
        &mut self,
        arrow: &swc_ast::ArrowExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let inherited_home_object = self.lexical_home_object;
        let outer_super_allowed = self.super_allowed;
        let outer_super_call_allowed = self.super_call_allowed;
        let name = format!("arrow_{}", self.module.functions().len());
        let async_name = format!("{name}$async");

        self.push_function_context(&async_name, BasicBlockId(0));
        self.is_async_fn = true;
        self.async_state_counter = 1;
        self.captured_var_slots.clear();
        self.async_resume_blocks.clear();
        self.is_arrow = true;
        *self.is_arrow_fn_stack.last_mut().unwrap() = true;
        self.lexical_home_object = inherited_home_object;
        self.super_allowed = outer_super_allowed;
        self.super_call_allowed = outer_super_call_allowed;
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;

        let state_scope_id = self
            .scopes
            .declare("$state", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;
        let resume_val_scope_id = self
            .scopes
            .declare("$resume_val", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;
        let is_rejected_scope_id = self
            .scopes
            .declare("$is_rejected", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;
        let promise_scope_id = self
            .scopes
            .declare("$promise", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;
        let closure_env_scope_id = self
            .scopes
            .declare("$closure_env", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;

        self.async_env_scope_id = env_scope_id;
        self.async_state_scope_id = state_scope_id;
        self.async_resume_val_scope_id = resume_val_scope_id;
        self.async_is_rejected_scope_id = is_rejected_scope_id;
        self.async_promise_scope_id = promise_scope_id;
        self.async_closure_env_ir_name = Some(format!("${closure_env_scope_id}.$closure_env"));

        let user_param_ir_names =
            self.build_arrow_param_ir_names(&arrow.params, env_scope_id, this_scope_id)?;
        self.init_async_continuation_slots(&user_param_ir_names, 4);

        let param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

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

        for (i, _param) in arrow.params.iter().enumerate() {
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
            self.emit_arrow_param_inits(&arrow.params, &user_param_ir_names, entry)?;

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

        let mut inner_flow;
        match arrow.body.as_ref() {
            swc_ast::BlockStmtOrExpr::BlockStmt(block_stmt) => {
                self.predeclare_block_stmts(&block_stmt.stmts)?;
                inner_flow = StmtFlow::Open(body_entry);
                for stmt in &block_stmt.stmts {
                    if matches!(inner_flow, StmtFlow::Terminated) {
                        continue;
                    }
                    inner_flow = self.lower_stmt(stmt, inner_flow)?;
                }
            }
            swc_ast::BlockStmtOrExpr::Expr(expr) => {
                let val = self.lower_expr(expr, body_entry)?;
                let promise_val = self.alloc_value();
                self.current_function.append_instruction(
                    body_entry,
                    Instruction::LoadVar {
                        dest: promise_val,
                        name: format!("${promise_scope_id}.$promise"),
                    },
                );
                self.current_function.append_instruction(
                    body_entry,
                    Instruction::PromiseResolve {
                        promise: promise_val,
                        value: val,
                    },
                );
                self.current_function
                    .set_terminator(body_entry, Terminator::Return { value: None });
                inner_flow = StmtFlow::Terminated;
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

        // ── 推迟的 save/restore：运行 liveness 分析并插入 save/restore ──
        self.resolve_pending_suspends();
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
        if let Some(span) = self.span_to_source_span(arrow.span()) {
            ir_function.set_source_span(span);
        }
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        if let Some(home) = inherited_home_object {
            ir_function.home_object = Some(home);
        }
        for b in blocks {
            ir_function.push_block(b);
        }
        let async_fn_id = self.module.push_function(ir_function);

        self.pop_function_context();

        self.push_function_context(&name, BasicBlockId(0));

        let wrapper_env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;
        let wrapper_this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(arrow.span, msg))?;

        let wrapper_user_param_ir_names = self.build_arrow_param_ir_names(
            &arrow.params,
            wrapper_env_scope_id,
            wrapper_this_scope_id,
        )?;

        let _wrapper_param_ir_names = [
            format!("${wrapper_env_scope_id}.$env"),
            format!("${wrapper_this_scope_id}.$this"),
        ];

        let wrapper_entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(wrapper_entry);

        let wrapper_after_inits = self.emit_arrow_param_inits(
            &arrow.params,
            &wrapper_user_param_ir_names,
            wrapper_entry,
        )?;

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

        for (i, _pat) in arrow.params.iter().enumerate() {
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
        if let Some(span) = self.span_to_source_span(arrow.span()) {
            wrapper_ir_function.set_source_span(span);
        }
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
            let mut closure_block = block;
            let env_val = self.ensure_shared_env(closure_block, &captured, arrow.span)?;
            closure_block = self.resolve_store_block(closure_block);
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                closure_block,
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
