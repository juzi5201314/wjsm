use super::*;

impl Lowerer {
    pub(crate) fn lower_stmt(
        &mut self,
        stmt: &swc_ast::Stmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        // Empty 不产生用户可见执行点，跳过 debug_check。
        if matches!(stmt, swc_ast::Stmt::Empty(_)) {
            return self.lower_empty(flow);
        }

        // 用户可见语句入口：在真正 lowering 前发射 DebugCheck（供单步/断点映射）。
        let flow = if self.emit_debug_checks {
            self.emit_stmt_debug_check(stmt, flow)?
        } else {
            flow
        };

        match stmt {
            swc_ast::Stmt::Expr(expr_stmt) => self.lower_expr_stmt(expr_stmt, flow),
            // N.B.: exhaustive match — new swc_ast::Decl variants must be handled here.
            swc_ast::Stmt::Decl(decl) => match decl {
                swc_ast::Decl::Fn(fn_decl) => self.lower_fn_decl(fn_decl, flow),
                swc_ast::Decl::Var(var_decl) => self.lower_var_decl(var_decl, flow),
                swc_ast::Decl::Class(class_decl) => self.lower_class_decl(class_decl, flow),
                swc_ast::Decl::TsInterface(_) => self.lower_empty(flow),
                swc_ast::Decl::TsTypeAlias(_) => self.lower_empty(flow),
                swc_ast::Decl::TsEnum(ts_enum) => self.lower_ts_enum(ts_enum, flow),
                swc_ast::Decl::TsModule(ts_module) => self.lower_ts_module(ts_module, flow),
                swc_ast::Decl::Using(using_decl) => self.lower_using_decl(using_decl, flow),
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
            swc_ast::Stmt::Empty(_) => unreachable!("Empty 已在 lower_stmt 入口处理"),
            swc_ast::Stmt::Debugger(_) => self.lower_debugger(flow),
            swc_ast::Stmt::With(with_stmt) => self.lower_with(with_stmt, flow),
        }
    }

    /// 在语句入口发射 `debug_check line=N col=M`；无源码上下文时跳过。
    fn emit_stmt_debug_check(
        &mut self,
        stmt: &swc_ast::Stmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        if let Some(span) = self.span_to_source_span(stmt.span()) {
            self.current_function.append_instruction(
                block,
                Instruction::DebugCheck {
                    line: span.line,
                    col: span.col,
                },
            );
        }
        Ok(StmtFlow::Open(self.resolve_store_block(block)))
    }

    // ── Expression statements ───────────────────────────────────────────────

    pub(crate) fn lower_expr_stmt(
        &mut self,
        expr_stmt: &swc_ast::ExprStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        if self.eval_mode {
            // 必须用 lower_expr_then_continue：EvalGetBinding 等会分叉异常块，
            // 若仅 lower_expr 再 resolve 起始 block，会丢掉 continue 并把 Return 盖掉 Branch。
            let mut current_block = block;
            let value = self.lower_expr_then_continue(&expr_stmt.expr, &mut current_block)?;
            self.eval_completion = Some(value);
            return Ok(StmtFlow::Open(current_block));
        }

        let result_block = match expr_stmt.expr.as_ref() {
            swc_ast::Expr::Call(call) => self.lower_call(call, block)?,
            expr => {
                let mut continuation = block;
                let value = self.lower_expr_then_continue(expr, &mut continuation)?;
                if self.expr_exception_fork_allowed() && self.expr_can_throw(expr) {
                    self.lower_value_exception_branch(continuation, value)?
                } else {
                    continuation
                }
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

    fn is_unqualified_direct_eval_call(call: &swc_ast::CallExpr) -> bool {
        matches!(
            &call.callee,
            swc_ast::Callee::Expr(expr)
                if matches!(expr.as_ref(), swc_ast::Expr::Ident(ident) if ident.sym.as_ref() == "eval")
        )
    }

    pub(crate) fn lower_call(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        let is_direct_eval =
            Self::is_unqualified_direct_eval_call(call) && self.scopes.lookup("eval").is_err();
        let result = self.lower_call_expr(call, block)?;

        // eval 调用已在 lower_direct_eval_call 内处理异常分叉，
        // 此时 block 可能已被终结；resolve_store_block 获取正确的继续块
        let working_block = self.resolve_store_block(block);

        if is_direct_eval {
            return Ok(working_block);
        }

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
        let mut cond_entry = block;

        let cond = if self.expr_exception_fork_allowed() && self.expr_can_throw(&if_stmt.test) {
            self.lower_expr_then_continue(&if_stmt.test, &mut cond_entry)?
        } else {
            self.lower_expr(&if_stmt.test, cond_entry)?
        };
        let branch_block =
            if self.expr_exception_fork_allowed() && self.expr_can_throw(&if_stmt.test) {
                let resolved = self.resolve_store_block(cond_entry);
                self.lower_value_exception_branch(resolved, cond)?
            } else {
                self.resolve_store_block(cond_entry)
            };
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
}
