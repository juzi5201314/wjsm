use super::*;

impl Lowerer {
    pub(crate) fn lower_stmt(
        &mut self,
        stmt: &swc_ast::Stmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        match stmt {
            swc_ast::Stmt::Expr(expr_stmt) => self.lower_expr_stmt(expr_stmt, flow),
            swc_ast::Stmt::Decl(decl) => match decl {
                swc_ast::Decl::Fn(fn_decl) => self.lower_fn_decl(fn_decl, flow),
                swc_ast::Decl::Var(var_decl) => self.lower_var_decl(var_decl, flow),
                swc_ast::Decl::Class(class_decl) => self.lower_class_decl(class_decl, flow),
                swc_ast::Decl::TsInterface(_) => self.lower_empty(flow),
                swc_ast::Decl::TsTypeAlias(_) => self.lower_empty(flow),
                swc_ast::Decl::TsEnum(ts_enum) => self.lower_ts_enum(ts_enum, flow),
                swc_ast::Decl::TsModule(ts_module) => self.lower_ts_module(ts_module, flow),
                swc_ast::Decl::Using(using_decl) => self.lower_using_decl(using_decl, flow),
                #[allow(unreachable_patterns)]
                _ => Err(self.error(
                    stmt.span(),
                    format!("unsupported declaration kind `{}`", decl_kind(decl)),
                )),
            },
            swc_ast::Stmt::Block(block_stmt) => self.lower_block_stmt(block_stmt, flow),
            swc_ast::Stmt::If(if_stmt) => self.lower_if(if_stmt, flow),
            swc_ast::Stmt::While(while_stmt) => self.lower_while(while_stmt, flow),
            swc_ast::Stmt::DoWhile(do_while_stmt) => self.lower_do_while(do_while_stmt, flow),
            swc_ast::Stmt::For(for_stmt) => self.lower_for(for_stmt, flow),
            swc_ast::Stmt::ForIn(for_in) => self.lower_for_in(for_in, flow),
            swc_ast::Stmt::ForOf(for_of) => self.lower_for_of(for_of, flow),
            swc_ast::Stmt::Break(break_stmt) => self.lower_break(break_stmt, flow),
            swc_ast::Stmt::Continue(continue_stmt) => self.lower_continue(continue_stmt, flow),
            swc_ast::Stmt::Return(return_stmt) => self.lower_return(return_stmt, flow),
            swc_ast::Stmt::Labeled(labeled) => self.lower_labeled(labeled, flow),
            swc_ast::Stmt::Switch(switch_stmt) => self.lower_switch(switch_stmt, flow),
            swc_ast::Stmt::Throw(throw_stmt) => self.lower_throw(throw_stmt, flow),
            swc_ast::Stmt::Try(try_stmt) => self.lower_try(try_stmt, flow),
            swc_ast::Stmt::Empty(_) => self.lower_empty(flow),
            swc_ast::Stmt::Debugger(_) => self.lower_debugger(flow),
            swc_ast::Stmt::With(with_stmt) => self.lower_with(with_stmt, flow),
        }
    }

    // ── Expression statements ───────────────────────────────────────────────

    pub(crate) fn lower_expr_stmt(
        &mut self,
        expr_stmt: &swc_ast::ExprStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        if self.eval_mode {
            let value = self.lower_expr(&expr_stmt.expr, block)?;
            self.eval_completion = Some(value);
            return Ok(StmtFlow::Open(self.resolve_store_block(block)));
        }

        let result_block = match expr_stmt.expr.as_ref() {
            swc_ast::Expr::Call(call) => self.lower_call(call, block)?,
            swc_ast::Expr::Member(_) | swc_ast::Expr::OptChain(_) => {
                let value = self.lower_expr(expr_stmt.expr.as_ref(), block)?;
                self.lower_value_exception_branch(block, value)?
            }
            expr => {
                let _value = self.lower_expr(expr, block)?;
                self.resolve_store_block(block)
            }
        };
        Ok(StmtFlow::Open(result_block))
    }

    pub(crate) fn lower_value_exception_branch(
        &mut self,
        block: BasicBlockId,
        value: ValueId,
    ) -> Result<BasicBlockId, LoweringError> {
        let working_block = self.resolve_store_block(block);
        let is_exception = self.alloc_value();
        self.current_function.append_instruction(
            working_block,
            Instruction::IsException {
                dest: is_exception,
                value,
            },
        );
        let continue_block = self.current_function.new_block();
        let exc_block = self.current_function.new_block();
        self.current_function.set_terminator(
            working_block,
            Terminator::Branch {
                condition: is_exception,
                true_block: exc_block,
                false_block: continue_block,
            },
        );
        let thrown_val = self.alloc_value();
        self.current_function.append_instruction(
            exc_block,
            Instruction::CallBuiltin {
                dest: Some(thrown_val),
                builtin: Builtin::ExceptionValue,
                args: vec![value],
            },
        );
        self.emit_throw_value(exc_block, thrown_val)?;
        Ok(self.resolve_store_block(continue_block))
    }

    pub(crate) fn lower_call(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        let result = self.lower_call_expr(call, block)?;

        // eval 调用已在 lower_direct_eval_call 内处理异常分叉，
        // 此时 block 可能已被终结；resolve_store_block 获取正确的继续块
        let working_block = self.resolve_store_block(block);

        // 跨函数异常检查：call 返回值可能是 TAG_EXCEPTION
        let is_exception = self.alloc_value();
        self.current_function.append_instruction(
            working_block,
            Instruction::IsException {
                dest: is_exception,
                value: result,
            },
        );
        let continue_block = self.current_function.new_block();
        let exc_block = self.current_function.new_block();
        self.current_function.set_terminator(
            working_block,
            Terminator::Branch {
                condition: is_exception,
                true_block: exc_block,
                false_block: continue_block,
            },
        );

        // 异常路径：解封装并传播
        let thrown_val = self.alloc_value();
        self.current_function.append_instruction(
            exc_block,
            Instruction::CallBuiltin {
                dest: Some(thrown_val),
                builtin: Builtin::ExceptionValue,
                args: vec![result],
            },
        );
        self.emit_throw_value(exc_block, thrown_val)?;

        // 返回继续 block
        Ok(self.resolve_store_block(continue_block))
    }
    // ── Blocks ──────────────────────────────────────────────────────────────

    pub(crate) fn lower_block_stmt(
        &mut self,
        block_stmt: &swc_ast::BlockStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let prev_using_count = self.active_using_vars.len();
        self.scopes.push_scope(ScopeKind::Block);
        self.predeclare_block_stmts(&block_stmt.stmts)?;

        let mut flow = flow;
        for stmt in &block_stmt.stmts {
            // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
            if matches!(flow, StmtFlow::Terminated) {
                continue;
            }
            flow = self.lower_stmt(stmt, flow)?;
        }

        // 在块退出时，对块内声明的 using 变量执行 dispose
        let new_using_count = self.active_using_vars.len();
        if new_using_count > prev_using_count {
            match flow {
                StmtFlow::Open(block) => {
                    let merged = self.emit_using_disposal(block);
                    self.active_using_vars.truncate(prev_using_count);
                    flow = StmtFlow::Open(merged);
                }
                StmtFlow::Terminated => {
                    // 块因 return/throw/break/continue 终止，
                    // using 变量的 dispose 由外层 finally 或运行时异常处理负责
                    self.active_using_vars.truncate(prev_using_count);
                }
            }
        }

        self.scopes.pop_scope();
        Ok(flow)
    }

    // ── if / else ───────────────────────────────────────────────────────────

    pub(crate) fn lower_if(
        &mut self,
        if_stmt: &swc_ast::IfStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        let cond = self.lower_expr(&if_stmt.test, block)?;
        let branch_block = self.resolve_store_block(block);
        let then_block = self.current_function.new_block();
        let else_or_merge = self.current_function.new_block();

        self.current_function.set_terminator(
            branch_block,
            Terminator::Branch {
                condition: cond,
                true_block: then_block,
                false_block: else_or_merge,
            },
        );

        let incoming_eval_completion = self.eval_completion;

        // lower 'then' branch
        let then_flow = self.lower_stmt(&if_stmt.cons, StmtFlow::Open(then_block))?;
        let then_eval_completion = self.eval_completion;

        let has_else = if let Some(alt) = &if_stmt.alt {
            self.eval_completion = incoming_eval_completion;
            // 'else' uses else_or_merge as its entry
            let else_flow = self.lower_stmt(alt, StmtFlow::Open(else_or_merge))?;
            let else_eval_completion = self.eval_completion;
            match (then_flow, else_flow) {
                (StmtFlow::Terminated, StmtFlow::Terminated) => StmtFlow::Terminated,
                _ => {
                    // Create a merge block only if at least one path doesn't terminate
                    let merge = self.current_function.new_block();
                    let after_then = self
                        .current_function
                        .ensure_jump_or_terminated(then_flow, merge);
                    let after_else = self
                        .current_function
                        .ensure_jump_or_terminated(else_flow, merge);
                    self.merge_eval_completion_after_if(
                        merge,
                        then_flow,
                        then_eval_completion,
                        after_then,
                        else_flow,
                        else_eval_completion,
                        after_else,
                    );
                    after_then
                }
            }
        } else {
            // No else: else_or_merge is the merge block (empty)
            // 即使 then 分支终止（break/return/continue），else 路径仍然可达
            let merge = else_or_merge;
            let _after_then = self
                .current_function
                .ensure_jump_or_terminated(then_flow, merge);
            if self.eval_mode {
                self.eval_completion = incoming_eval_completion.or(then_eval_completion);
            }
            StmtFlow::Open(merge)
        };

        Ok(has_else)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn merge_eval_completion_after_if(
        &mut self,
        merge: BasicBlockId,
        then_flow: StmtFlow,
        then_eval_completion: Option<ValueId>,
        _after_then: StmtFlow,
        else_flow: StmtFlow,
        else_eval_completion: Option<ValueId>,
        _after_else: StmtFlow,
    ) {
        if !self.eval_mode {
            return;
        }

        let (StmtFlow::Open(then_block), Some(then_value)) = (then_flow, then_eval_completion)
        else {
            self.eval_completion = then_eval_completion.or(else_eval_completion);
            return;
        };
        let (StmtFlow::Open(else_block), Some(else_value)) = (else_flow, else_eval_completion)
        else {
            self.eval_completion = then_eval_completion.or(else_eval_completion);
            return;
        };

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: then_block,
                        value: then_value,
                    },
                    PhiSource {
                        predecessor: else_block,
                        value: else_value,
                    },
                ],
            },
        );
        self.eval_completion = Some(result);
    }

    // ── while ───────────────────────────────────────────────────────────────

    pub(crate) fn lower_while(
        &mut self,
        while_stmt: &swc_ast::WhileStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        let header = self.current_function.new_block();
        let body = self.current_function.new_block();
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(block, Terminator::Jump { target: header });
        let true_val = self.alloc_value();
        let true_const = self.module.add_constant(Constant::Bool(true));
        self.current_function.append_instruction(
            header,
            Instruction::Const {
                dest: true_val,
                constant: true_const,
            },
        );

        let cond = self.lower_expr(&while_stmt.test, header)?;
        let branch_header = self.resolve_store_block(header);
        self.current_function.set_terminator(
            branch_header,
            Terminator::Branch {
                condition: cond,
                true_block: body,
                false_block: exit,
            },
        );

        self.label_stack.push(LabelContext {
            label: self.pending_loop_label.take(),
            kind: LabelKind::Loop,
            break_target: exit,
            continue_target: Some(header),
            iterator_to_close: None,
        });

        let body_flow = self.lower_stmt(&while_stmt.body, StmtFlow::Open(body))?;
        let _ = self
            .current_function
            .ensure_jump_or_terminated(body_flow, header);

        self.label_stack.pop();

        Ok(StmtFlow::Open(exit))
    }

    // ── do...while ──────────────────────────────────────────────────────────

    pub(crate) fn lower_do_while(
        &mut self,
        do_while: &swc_ast::DoWhileStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        let body = self.current_function.new_block();
        let condition = self.current_function.new_block();
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(block, Terminator::Jump { target: body });

        self.label_stack.push(LabelContext {
            label: self.pending_loop_label.take(),
            kind: LabelKind::Loop,
            break_target: exit,
            continue_target: Some(condition),
            iterator_to_close: None,
        });

        let body_flow = self.lower_stmt(&do_while.body, StmtFlow::Open(body))?;
        let _ = self
            .current_function
            .ensure_jump_or_terminated(body_flow, condition);

        let cond = self.lower_expr(&do_while.test, condition)?;
        let branch_condition = self.resolve_store_block(condition);
        self.current_function.set_terminator(
            branch_condition,
            Terminator::Branch {
                condition: cond,
                true_block: body,
                false_block: exit,
            },
        );

        self.label_stack.pop();

        Ok(StmtFlow::Open(exit))
    }

    // ── for ─────────────────────────────────────────────────────────────────

    pub(crate) fn lower_for(
        &mut self,
        for_stmt: &swc_ast::ForStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        // init
        if let Some(init) = &for_stmt.init {
            match init {
                swc_ast::VarDeclOrExpr::VarDecl(var_decl) => {
                    self.lower_var_decl(var_decl, StmtFlow::Open(block))?;
                }
                swc_ast::VarDeclOrExpr::Expr(expr) => {
                    let _ = self.lower_expr(expr, block)?;
                }
            }
        }

        let init_end = self.resolve_store_block(block);
        let header = self.current_function.new_block();
        let body_block = self.current_function.new_block();
        let update = self.current_function.new_block();
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(init_end, Terminator::Jump { target: header });

        self.label_stack.push(LabelContext {
            label: self.pending_loop_label.take(),
            kind: LabelKind::Loop,
            break_target: exit,
            continue_target: Some(update),
            iterator_to_close: None,
        });

        // condition
        if let Some(test) = &for_stmt.test {
            let cond = self.lower_expr(test, header)?;
            let branch_header = self.resolve_store_block(header);
            self.current_function.set_terminator(
                branch_header,
                Terminator::Branch {
                    condition: cond,
                    true_block: body_block,
                    false_block: exit,
                },
            );
        } else {
            // no condition → always true
            let true_val = self.load_bool_constant(true, header);
            self.current_function.set_terminator(
                header,
                Terminator::Branch {
                    condition: true_val,
                    true_block: body_block,
                    false_block: exit,
                },
            );
        }

        // body
        let body_flow = self.lower_stmt(&for_stmt.body, StmtFlow::Open(body_block))?;
        let _ = self
            .current_function
            .ensure_jump_or_terminated(body_flow, update);

        // update
        if let Some(update_expr) = &for_stmt.update {
            let _ = self.lower_expr(update_expr, update)?;
        }
        let update_end = self.resolve_store_block(update);
        self.current_function
            .set_terminator(update_end, Terminator::Jump { target: header });

        self.label_stack.pop();

        Ok(StmtFlow::Open(exit))
    }

    // ── for...in ────────────────────────────────────────────────────────────

    pub(crate) fn lower_for_in(
        &mut self,
        for_in: &swc_ast::ForInStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        let rhs = self.lower_expr(&for_in.right, block)?;

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
            iterator_to_close: None,
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

        self.lower_for_in_of_lhs(&for_in.left, key_val, body_block)?;

        let body_flow = self.lower_stmt(&for_in.body, StmtFlow::Open(body_block))?;
        let _ = self
            .current_function
            .ensure_jump_or_terminated(body_flow, next);

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
        let block = self.ensure_open(flow)?;

        let iterable = self.lower_expr(&for_of.right, block)?;

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

        let header = self.current_function.new_block();
        let body_block = self.current_function.new_block();
        let next_block = self.current_function.new_block();
        let close = self.current_function.new_block();
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
        // 反转 done 条件：backend 假设 loop condition "true = continue",
        // 但 done_val 是 "true = done = exit"。使用 Not 反转。
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

        // Register label context: break → close (which then jumps to exit)
        self.label_stack.push(LabelContext {
            label: self.pending_loop_label.take(),
            kind: LabelKind::Loop,
            break_target: close,
            continue_target: Some(next_block),
            iterator_to_close: Some(iter_handle),
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

        self.lower_for_in_of_lhs(&for_of.left, value_val, body_block)?;

        let body_flow = self.lower_stmt(&for_of.body, StmtFlow::Open(body_block))?;
        let _ = self
            .current_function
            .ensure_jump_or_terminated(body_flow, next_block);

        self.label_stack.pop();

        // close block: iterator clean-up on break
        self.current_function.append_instruction(
            close,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::IteratorClose,
                args: vec![iter_handle],
            },
        );
        self.current_function
            .set_terminator(close, Terminator::Jump { target: exit });

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
        // break/continue/return/throw 走 abrupt 路径；正常完成与 break→close→exit 均以 Open(exit) 结束，
        // 使 exit 块得到 Return 而非 Unreachable（for-of break 关闭迭代器场景）。
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

        let block = self.ensure_open(flow)?;

        let iterable = self.lower_expr(&for_of.right, block)?;

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

        self.lower_for_in_of_lhs(&for_of.left, value_val, body_block)?;

        let body_flow = self.lower_stmt(&for_of.body, StmtFlow::Open(body_block))?;
        let _ = self
            .current_function
            .ensure_jump_or_terminated(body_flow, header);

        self.label_stack.pop();

        let iter_for_close = self.alloc_value();
        self.current_function.append_instruction(
            close,
            Instruction::LoadVar {
                dest: iter_for_close,
                name: iter_ir_name,
            },
        );
        self.current_function.append_instruction(
            close,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::IteratorClose,
                args: vec![iter_for_close],
            },
        );
        self.current_function
            .set_terminator(close, Terminator::Jump { target: exit });

        Ok(StmtFlow::Open(exit))
    }

    /// Lower the LHS of a for...in or for...of loop.
    /// Supports: simple identifier, or var declaration with single binding identifier.
    pub(crate) fn lower_for_in_of_lhs(
        &mut self,
        left: &swc_ast::ForHead,
        value: ValueId,
        block: BasicBlockId,
    ) -> Result<(), LoweringError> {
        match left {
            swc_ast::ForHead::Pat(pat) => match &**pat {
                swc_ast::Pat::Ident(binding) => {
                    let name = binding.id.sym.to_string();
                    let (scope_id, _) = self
                        .scopes
                        .lookup(&name)
                        .map_err(|msg| self.error(pat.span(), msg))?;
                    let ir_name = format!("${scope_id}.{name}");
                    self.current_function.append_instruction(
                        block,
                        Instruction::StoreVar {
                            name: ir_name,
                            value,
                        },
                    );
                    Ok(())
                }
                swc_ast::Pat::Object(_) | swc_ast::Pat::Array(_) | swc_ast::Pat::Assign(_) => {
                    Err(self.error(
                        pat.span(),
                        "destructuring patterns in for...in/for...of are not yet supported",
                    ))
                }
                _ => Err(self.error(
                    pat.span(),
                    "destructuring patterns in for...in/for...of are not yet supported",
                )),
            },
            swc_ast::ForHead::VarDecl(var_decl) => {
                let kind = match var_decl.kind {
                    swc_ast::VarDeclKind::Var => VarKind::Var,
                    swc_ast::VarDeclKind::Let => VarKind::Let,
                    swc_ast::VarDeclKind::Const => VarKind::Const,
                };
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
                            let ir_name = format!("${scope_id}.{name}");
                            self.current_function.append_instruction(
                                block,
                                Instruction::StoreVar {
                                    name: ir_name,
                                    value,
                                },
                            );
                        }
                        _ => {
                            self.lower_destructure_pattern(&declarator.name, value, block, kind)?;
                        }
                    }
                }
                Ok(())
            }
            swc_ast::ForHead::UsingDecl(_) => Err(self.error(
                DUMMY_SP,
                "using declarations in for...in/for...of are not yet supported",
            )),
        }
    }

    // ── break / continue ────────────────────────────────────────────────────
}
