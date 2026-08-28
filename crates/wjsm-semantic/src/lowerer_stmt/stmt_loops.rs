use super::*;

impl Lowerer {
    /// `for (let x in/of ...)` 的头部绑定属于本循环自有的词法作用域
    /// （ES2015 13.7.5）：相邻两个循环各自持有独立绑定，不构成重复声明。
    /// 头部作用域必须在此压栈，因为 predeclare 只提升 `var`。
    fn for_head_needs_scope(left: &swc_ast::ForHead) -> bool {
        matches!(left, swc_ast::ForHead::VarDecl(decl) if decl.kind != swc_ast::VarDeclKind::Var)
    }

    /// 在头部作用域内声明 `let`/`const` 绑定（`var` 已由 predeclare 提升）。
    fn declare_for_head_bindings(&mut self, left: &swc_ast::ForHead) -> Result<(), LoweringError> {
        if let swc_ast::ForHead::VarDecl(var_decl) = left
            && var_decl.kind != swc_ast::VarDeclKind::Var
        {
            self.predeclare_var_decl(var_decl)?;
        }
        Ok(())
    }

    pub(crate) fn lower_for_in(
        &mut self,
        for_in: &swc_ast::ForInStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let head_scope = Self::for_head_needs_scope(&for_in.left);
        if head_scope {
            self.scopes.push_scope(ScopeKind::Block);
        }
        let result = self.lower_for_in_scoped(for_in, flow);
        if head_scope {
            self.scopes.pop_scope();
        }
        result
    }

    fn lower_for_in_scoped(
        &mut self,
        for_in: &swc_ast::ForInStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        self.declare_for_head_bindings(&for_in.left)?;
        let captured = self.loop_body_captured_bindings(&for_in.body);
        let iteration_bindings = match &for_in.left {
            swc_ast::ForHead::VarDecl(declaration) => {
                self.loop_head_iteration_bindings(declaration, &captured, true)?
            }
            _ => Vec::new(),
        };
        self.mark_stable_loop_captures(&captured, &iteration_bindings);
        let mut block = self.ensure_open(flow)?;
        block = self.initialize_stable_loop_captures(block, &captured, &iteration_bindings)?;

        let rhs = self.lower_expr(&for_in.right, block)?;
        block = self.resolve_store_block(block);

        // Create enumerator from object
        let enum_handle = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(enum_handle),
                builtin: Builtin::EnumeratorFrom,
                args: vec![rhs],
            },
        );
        let iteration_frame = if iteration_bindings.is_empty() {
            None
        } else {
            let (continuation, frame) = self.prepare_iteration_env(block, iteration_bindings)?;
            block = continuation;
            Some(frame)
        };

        let header = self.current_function.new_block();
        let body_block = self.current_function.new_block();
        let next = self.current_function.new_block();
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(block, Terminator::Jump { target: header });

        // header: check enumerator done
        let done_val = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::CallBuiltin {
                dest: Some(done_val),
                builtin: Builtin::EnumeratorDone,
                args: vec![enum_handle],
            },
        );
        // 反转 done 条件：backend 假设 loop condition "true = continue",
        let not_done = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::Unary {
                dest: not_done,
                op: UnaryOp::Not,
                value: done_val,
            },
        );
        self.current_function.set_terminator(
            header,
            Terminator::Branch {
                condition: not_done,
                true_block: body_block,
                false_block: exit,
            },
        );

        self.label_stack.push(LabelContext {
            label: self.pending_loop_label.take(),
            kind: LabelKind::Loop,
            break_target: exit,
            continue_target: Some(next),
            iterator_to_close: Some(enum_handle),
            break_reached: false,
        });

        // body: get key, assign lhs
        let key_val = self.alloc_value();
        self.current_function.append_instruction(
            body_block,
            Instruction::CallBuiltin {
                dest: Some(key_val),
                builtin: Builtin::EnumeratorKey,
                args: vec![enum_handle],
            },
        );

        if let Some(frame) = &iteration_frame {
            self.initialize_empty_iteration_env(body_block, frame);
            self.iteration_env_stack.push(frame.clone());
        }
        let body_entry = self.lower_for_in_of_lhs(&for_in.left, key_val, body_block)?;

        let body_flow = self.lower_stmt(&for_in.body, StmtFlow::Open(body_entry))?;
        let _ = self
            .current_function
            .ensure_jump_or_terminated(body_flow, next);
        if iteration_frame.is_some() {
            self.iteration_env_stack
                .pop()
                .expect("iteration env stack underflow");
        }

        // next
        self.current_function.append_instruction(
            next,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::EnumeratorNext,
                args: vec![enum_handle],
            },
        );
        self.current_function
            .set_terminator(next, Terminator::Jump { target: header });

        self.label_stack.pop();

        Ok(StmtFlow::Open(exit))
    }

    // ── for...of ────────────────────────────────────────────────────────────

    pub(crate) fn lower_for_of(
        &mut self,
        for_of: &swc_ast::ForOfStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        if for_of.is_await {
            return self.lower_for_await_of(for_of, flow);
        }
        let head_scope = Self::for_head_needs_scope(&for_of.left);
        if head_scope {
            self.scopes.push_scope(ScopeKind::Block);
        }
        let result = self.lower_for_of_scoped(for_of, flow);
        if head_scope {
            self.scopes.pop_scope();
        }
        result
    }

    fn lower_for_of_scoped(
        &mut self,
        for_of: &swc_ast::ForOfStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        self.declare_for_head_bindings(&for_of.left)?;
        let captured = self.loop_body_captured_bindings(&for_of.body);
        let iteration_bindings = match &for_of.left {
            swc_ast::ForHead::VarDecl(declaration) => {
                self.loop_head_iteration_bindings(declaration, &captured, true)?
            }
            _ => Vec::new(),
        };
        self.mark_stable_loop_captures(&captured, &iteration_bindings);
        let mut block = self.ensure_open(flow)?;
        block = self.initialize_stable_loop_captures(block, &captured, &iteration_bindings)?;

        let iterable = self.lower_expr(&for_of.right, block)?;
        block = self.resolve_store_block(block);

        // Create iterator from iterable
        let iter_handle = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(iter_handle),
                builtin: Builtin::IteratorFrom,
                args: vec![iterable],
            },
        );
        block = self.lower_value_exception_branch(block, iter_handle)?;
        let iteration_frame = if iteration_bindings.is_empty() {
            None
        } else {
            let (continuation, frame) = self.prepare_iteration_env(block, iteration_bindings)?;
            block = continuation;
            Some(frame)
        };

        let header = self.current_function.new_block();
        let body_block = self.current_function.new_block();
        let next_block = self.current_function.new_block();
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(block, Terminator::Jump { target: header });

        // header: check iterator done
        let done_val = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::CallBuiltin {
                dest: Some(done_val),
                builtin: Builtin::IteratorDone,
                args: vec![iter_handle],
            },
        );
        let condition_block = self.lower_value_exception_branch(header, done_val)?;
        // 反转 done 条件：backend 假设 loop condition "true = continue"，
        // 但 done_val 是 "true = done = exit"。使用 Not 反转。
        let not_done = self.alloc_value();
        self.current_function.append_instruction(
            condition_block,
            Instruction::Unary {
                dest: not_done,
                op: UnaryOp::Not,
                value: done_val,
            },
        );
        self.current_function.set_terminator(
            condition_block,
            Terminator::Branch {
                condition: not_done,
                true_block: body_block,
                false_block: exit,
            },
        );

        // Register label context: break → exit（IteratorClose 由 emit_unwind_for_abrupt 发射）
        self.label_stack.push(LabelContext {
            label: self.pending_loop_label.take(),
            kind: LabelKind::Loop,
            break_target: exit,
            continue_target: Some(next_block),
            iterator_to_close: Some(iter_handle),
            break_reached: false,
        });

        // body: get value, assign lhs
        let value_val = self.alloc_value();
        self.current_function.append_instruction(
            body_block,
            Instruction::CallBuiltin {
                dest: Some(value_val),
                builtin: Builtin::IteratorValue,
                args: vec![iter_handle],
            },
        );

        if let Some(frame) = &iteration_frame {
            self.initialize_empty_iteration_env(body_block, frame);
            self.iteration_env_stack.push(frame.clone());
        }
        let body_entry = self.lower_for_in_of_lhs(&for_of.left, value_val, body_block)?;

        let body_flow = self.lower_stmt(&for_of.body, StmtFlow::Open(body_entry))?;
        let _ = self
            .current_function
            .ensure_jump_or_terminated(body_flow, next_block);
        if iteration_frame.is_some() {
            self.iteration_env_stack
                .pop()
                .expect("iteration env stack underflow");
        }

        self.label_stack.pop();

        // next: advance iterator
        self.current_function.append_instruction(
            next_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::IteratorNext,
                args: vec![iter_handle],
            },
        );
        self.current_function
            .set_terminator(next_block, Terminator::Jump { target: header });
        let _ = body_flow;
        Ok(StmtFlow::Open(exit))
    }

    pub(crate) fn lower_for_await_of(
        &mut self,
        for_of: &swc_ast::ForOfStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        if !self.is_async_fn {
            return Err(self.error(
                for_of.span(),
                "for await...of is only valid in async functions",
            ));
        }
        let head_scope = Self::for_head_needs_scope(&for_of.left);
        if head_scope {
            self.scopes.push_scope(ScopeKind::Block);
        }
        let result = self.lower_for_await_of_scoped(for_of, flow);
        if head_scope {
            self.scopes.pop_scope();
        }
        result
    }

    fn lower_for_await_of_scoped(
        &mut self,
        for_of: &swc_ast::ForOfStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        self.declare_for_head_bindings(&for_of.left)?;
        let captured = self.loop_body_captured_bindings(&for_of.body);
        let iteration_bindings = match &for_of.left {
            swc_ast::ForHead::VarDecl(declaration) => {
                self.loop_head_iteration_bindings(declaration, &captured, true)?
            }
            _ => Vec::new(),
        };
        self.mark_stable_loop_captures(&captured, &iteration_bindings);
        let mut block = self.ensure_open(flow)?;
        block = self.initialize_stable_loop_captures(block, &captured, &iteration_bindings)?;

        let iterable = self.lower_expr(&for_of.right, block)?;
        block = self.resolve_store_block(block);

        let iter_handle = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(iter_handle),
                builtin: Builtin::AsyncIteratorFrom,
                args: vec![iterable],
            },
        );
        let iter_binding = format!("$for_await_iter.{}", self.next_temp);
        self.next_temp += 1;
        let iter_scope_id = self
            .scopes
            .declare(&iter_binding, VarKind::Let, true)
            .map_err(|msg| self.error(for_of.span(), msg))?;
        let iter_ir_name = format!("${iter_scope_id}.{iter_binding}");
        self.current_function.append_instruction(
            block,
            Instruction::StoreVar {
                name: iter_ir_name.clone(),
                value: iter_handle,
            },
        );
        let iteration_frame = if iteration_bindings.is_empty() {
            None
        } else {
            let (continuation, frame) = self.prepare_iteration_env(block, iteration_bindings)?;
            block = continuation;
            Some(frame)
        };

        let header = self.current_function.new_block();
        let body_block = self.current_function.new_block();
        let close = self.current_function.new_block();
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(block, Terminator::Jump { target: header });

        let iter_for_next = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::LoadVar {
                dest: iter_for_next,
                name: iter_ir_name.clone(),
            },
        );
        let next_call_result = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::CallBuiltin {
                dest: Some(next_call_result),
                builtin: Builtin::IteratorNext,
                args: vec![iter_for_next],
            },
        );

        // ES spec: for-await 的迭代值需要通过 Promise.resolve 包装
        // 当 next() 返回非 Promise 值（如普通对象）时，Suspend 需要一个真正的 Promise
        let promised = self.alloc_value();
        {
            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                header,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );
            self.current_function.append_instruction(
                header,
                Instruction::CallBuiltin {
                    dest: Some(promised),
                    builtin: Builtin::PromiseResolveStatic,
                    args: vec![undef_val, next_call_result],
                },
            );
        }
        let next_result = promised;
        let next_state = self.async_state_counter;
        self.async_state_counter += 1;

        let resume_block = self.current_function.new_block();
        self.async_resume_blocks.push((next_state, resume_block));
        let visible_bindings = self.async_visible_binding_names();

        self.pending_suspends
            .push(lowerer_async_eval::PendingSuspend {
                suspend_block: header,
                resume_block,
                visible_bindings,
            });

        self.current_function.append_instruction(
            header,
            Instruction::Suspend {
                promise: next_result,
                state: next_state,
            },
        );

        let continue_after_await = self.current_function.new_block();
        self.current_function.set_terminator(
            header,
            Terminator::Jump {
                target: continue_after_await,
            },
        );

        let resume_val = self.alloc_value();
        self.current_function.append_instruction(
            resume_block,
            Instruction::LoadVar {
                dest: resume_val,
                name: format!("${}.$resume_val", self.async_resume_val_scope_id),
            },
        );
        let is_rejected = self.alloc_value();
        self.current_function.append_instruction(
            resume_block,
            Instruction::LoadVar {
                dest: is_rejected,
                name: format!("${}.$is_rejected", self.async_is_rejected_scope_id),
            },
        );
        let throw_block = self.current_function.new_block();
        self.current_function.set_terminator(
            resume_block,
            Terminator::Branch {
                condition: is_rejected,
                true_block: throw_block,
                false_block: continue_after_await,
            },
        );
        self.emit_throw_value(throw_block, resume_val)?;

        let awaited_result = self.alloc_value();
        self.current_function.append_instruction(
            continue_after_await,
            Instruction::LoadVar {
                dest: awaited_result,
                name: format!("${}.$resume_val", self.async_resume_val_scope_id),
            },
        );
        let done_key_const = self
            .module
            .add_constant(Constant::String("done".to_string()));
        let done_key = self.alloc_value();
        self.current_function.append_instruction(
            continue_after_await,
            Instruction::Const {
                dest: done_key,
                constant: done_key_const,
            },
        );
        let done_val = self.alloc_value();
        self.current_function.append_instruction(
            continue_after_await,
            Instruction::GetProp {
                dest: done_val,
                object: awaited_result,
                key: done_key,
            },
        );
        let not_done = self.alloc_value();
        self.current_function.append_instruction(
            continue_after_await,
            Instruction::Unary {
                dest: not_done,
                op: UnaryOp::Not,
                value: done_val,
            },
        );
        self.current_function.set_terminator(
            continue_after_await,
            Terminator::Branch {
                condition: not_done,
                true_block: body_block,
                false_block: exit,
            },
        );

        let iter_for_body_close = self.alloc_value();
        self.current_function.append_instruction(
            body_block,
            Instruction::LoadVar {
                dest: iter_for_body_close,
                name: iter_ir_name.clone(),
            },
        );
        self.label_stack.push(LabelContext {
            label: self.pending_loop_label.take(),
            kind: LabelKind::Loop,
            break_target: close,
            continue_target: Some(header),
            iterator_to_close: Some(iter_for_body_close),
            break_reached: false,
        });

        let awaited_result_for_value = self.alloc_value();
        self.current_function.append_instruction(
            body_block,
            Instruction::LoadVar {
                dest: awaited_result_for_value,
                name: format!("${}.$resume_val", self.async_resume_val_scope_id),
            },
        );
        let value_key_const = self
            .module
            .add_constant(Constant::String("value".to_string()));
        let value_key = self.alloc_value();
        self.current_function.append_instruction(
            body_block,
            Instruction::Const {
                dest: value_key,
                constant: value_key_const,
            },
        );
        let value_val = self.alloc_value();
        self.current_function.append_instruction(
            body_block,
            Instruction::GetProp {
                dest: value_val,
                object: awaited_result_for_value,
                key: value_key,
            },
        );

        if let Some(frame) = &iteration_frame {
            self.initialize_empty_iteration_env(body_block, frame);
            self.iteration_env_stack.push(frame.clone());
        }
        let body_entry = self.lower_for_in_of_lhs(&for_of.left, value_val, body_block)?;

        let body_flow = self.lower_stmt(&for_of.body, StmtFlow::Open(body_entry))?;
        let _ = self
            .current_function
            .ensure_jump_or_terminated(body_flow, header);
        if iteration_frame.is_some() {
            self.iteration_env_stack
                .pop()
                .expect("iteration env stack underflow");
        }

        self.label_stack.pop();

        let iter_for_close = self.alloc_value();
        self.current_function.append_instruction(
            close,
            Instruction::LoadVar {
                dest: iter_for_close,
                name: iter_ir_name,
            },
        );
        let after_close = self.emit_single_iterator_close_normal(close, iter_for_close)?;
        self.current_function
            .set_terminator(after_close, Terminator::Jump { target: exit });

        Ok(StmtFlow::Open(exit))
    }

    /// Lower the LHS of a for...in or for...of loop.
    /// Supports: identifier, assignment/destructuring patterns, or var declaration (incl. destructuring).
    pub(crate) fn lower_for_in_of_lhs(
        &mut self,
        left: &swc_ast::ForHead,
        value: ValueId,
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        match left {
            swc_ast::ForHead::Pat(pat) => match &**pat {
                swc_ast::Pat::Ident(binding) => {
                    let name = binding.id.sym.to_string();
                    // 解析穿越 with 作用域：每次迭代的头部写入按对象环境记录分派。
                    let crossed = self.with_scopes_for_ident(&name);
                    if !crossed.is_empty() {
                        return self.lower_with_bare_write(
                            &name,
                            pat.span(),
                            value,
                            &crossed,
                            block,
                        );
                    }
                    let (scope_id, _) = self
                        .scopes
                        .lookup(&name)
                        .map_err(|message| self.error(pat.span(), message))?;
                    let binding = CapturedBinding::new(name, scope_id);
                    self.store_binding_value(block, &binding, value, pat.span(), true)
                }
                swc_ast::Pat::Object(_) | swc_ast::Pat::Array(_) | swc_ast::Pat::Assign(_) => {
                    self.lower_destructure_pattern(pat, value, block, VarKind::Let)
                }
                _ => Err(self.error(pat.span(), "unsupported pattern in for...in/for...of")),
            },
            swc_ast::ForHead::VarDecl(var_decl) => {
                let kind = match var_decl.kind {
                    swc_ast::VarDeclKind::Var => VarKind::Var,
                    swc_ast::VarDeclKind::Let => VarKind::Let,
                    swc_ast::VarDeclKind::Const => VarKind::Const,
                };
                let mut block = block;
                for declarator in &var_decl.decls {
                    match &declarator.name {
                        swc_ast::Pat::Ident(binding) => {
                            let name = binding.id.sym.to_string();
                            let scope_id = self
                                .scopes
                                .resolve_scope_id(&name)
                                .map_err(|msg| self.error(var_decl.span, msg))?;
                            self.scopes
                                .mark_initialised(&name)
                                .map_err(|msg| self.error(var_decl.span, msg))?;
                            let binding = CapturedBinding::new(name, scope_id);
                            block = self.store_binding_value(
                                block,
                                &binding,
                                value,
                                var_decl.span,
                                true,
                            )?;
                        }
                        _ => {
                            block = self.lower_destructure_pattern(
                                &declarator.name,
                                value,
                                block,
                                kind,
                            )?;
                        }
                    }
                }
                Ok(block)
            }
            swc_ast::ForHead::UsingDecl(_) => Err(self.error(
                DUMMY_SP,
                "using declarations in for...in/for...of are not yet supported",
            )),
        }
    }

    // ── break / continue ────────────────────────────────────────────────────
}
