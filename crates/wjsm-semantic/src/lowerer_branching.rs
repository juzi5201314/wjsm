use super::*;

impl Lowerer {
    pub(crate) fn lower_break(
        &mut self,
        break_stmt: &swc_ast::BreakStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        let target_index = if let Some(label) = &break_stmt.label {
            self.find_label_context_index(label.sym.as_ref(), Some(label.span))?
        } else {
            self.find_nearest_break_context_index(break_stmt.span())?
        };
        let target = self.label_stack[target_index].break_target;
        let iterator_cleanups = self.iterator_cleanups_crossing(target_index);

        match self.lower_pending_finalizers(block)? {
            StmtFlow::Open(after_finally) => {
                let completion = self.alloc_undefined_value(after_finally);
                let after_close =
                    self.emit_iterator_closes(after_finally, &iterator_cleanups, completion)?;
                self.current_function
                    .set_terminator(after_close, Terminator::Jump { target });
            }
            StmtFlow::Terminated => {}
        }
        Ok(StmtFlow::Terminated)
    }

    pub(crate) fn lower_continue(
        &mut self,
        continue_stmt: &swc_ast::ContinueStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        let target_index = if let Some(label) = &continue_stmt.label {
            let index = self.find_label_context_index(label.sym.as_ref(), Some(label.span))?;
            if self.label_stack[index].continue_target.is_none() {
                return Err(self.error(
                    continue_stmt.span(),
                    format!("cannot continue to non-loop label `{}`", label.sym),
                ));
            }
            index
        } else {
            self.find_nearest_continue_context_index(continue_stmt.span())?
        };
        let target = self.label_stack[target_index]
            .continue_target
            .expect("continue target checked above");
        let iterator_cleanups = self.iterator_cleanups_crossing(target_index);

        match self.lower_pending_finalizers(block)? {
            StmtFlow::Open(after_finally) => {
                let completion = self.alloc_undefined_value(after_finally);
                let after_close =
                    self.emit_iterator_closes(after_finally, &iterator_cleanups, completion)?;
                self.current_function
                    .set_terminator(after_close, Terminator::Jump { target });
            }
            StmtFlow::Terminated => {}
        }
        Ok(StmtFlow::Terminated)
    }

    pub(crate) fn find_nearest_break_context_index(
        &self,
        span: Span,
    ) -> Result<usize, LoweringError> {
        for (index, ctx) in self.label_stack.iter().enumerate().rev() {
            if matches!(
                ctx.kind,
                LabelKind::Loop | LabelKind::Switch | LabelKind::Block
            ) {
                return Ok(index);
            }
        }
        Err(LoweringError::Diagnostic(Diagnostic::new(
            span.lo.0,
            span.hi.0,
            "break outside of loop or switch",
        )))
    }

    pub(crate) fn find_nearest_continue_context_index(
        &self,
        span: Span,
    ) -> Result<usize, LoweringError> {
        for (index, ctx) in self.label_stack.iter().enumerate().rev() {
            if ctx.continue_target.is_some() {
                return Ok(index);
            }
        }
        Err(LoweringError::Diagnostic(Diagnostic::new(
            span.lo.0,
            span.hi.0,
            "continue outside of loop",
        )))
    }

    pub(crate) fn find_label_context_index(
        &self,
        name: &str,
        error_span: Option<Span>,
    ) -> Result<usize, LoweringError> {
        for (index, ctx) in self.label_stack.iter().enumerate().rev() {
            if ctx.label.as_deref() == Some(name) {
                return Ok(index);
            }
        }
        let (start, end) = match error_span {
            Some(span) => (span.lo.0, span.hi.0),
            None => (0, 0),
        };
        Err(LoweringError::Diagnostic(Diagnostic::new(
            start,
            end,
            format!("unknown label `{name}`"),
        )))
    }

    pub(crate) fn iterator_cleanups_crossing(&self, target_index: usize) -> Vec<ValueId> {
        let mut iterators = self
            .label_stack
            .iter()
            .skip(target_index + 1)
            .filter_map(|ctx| ctx.iterator_to_close)
            .collect::<Vec<_>>();
        iterators.reverse();
        iterators
    }

    pub(crate) fn iterator_cleanups_from_depth(&self, depth: usize) -> Vec<ValueId> {
        let mut iterators = self
            .label_stack
            .iter()
            .skip(depth)
            .filter_map(|ctx| ctx.iterator_to_close)
            .collect::<Vec<_>>();
        iterators.reverse();
        iterators
    }

    pub(crate) fn active_iterator_cleanups(&self) -> Vec<ValueId> {
        self.iterator_cleanups_from_depth(0)
    }

    pub(crate) fn alloc_undefined_value(&mut self, block: BasicBlockId) -> ValueId {
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
    }

    /// 按 ES §7.4.6 关闭迭代器；`completion` 为 abrupt 时的完成值（正常关闭传 undefined）。
    pub(crate) fn emit_iterator_closes(
        &mut self,
        block: BasicBlockId,
        iterators: &[ValueId],
        completion: ValueId,
    ) -> Result<BasicBlockId, LoweringError> {
        let mut current = block;
        for iterator in iterators {
            current = self.resolve_store_block(current);
            let close_result = self.alloc_value();
            self.current_function.append_instruction(
                current,
                Instruction::CallBuiltin {
                    dest: Some(close_result),
                    builtin: Builtin::IteratorClose,
                    args: vec![*iterator, completion],
                },
            );
            let is_exception = self.alloc_value();
            self.current_function.append_instruction(
                current,
                Instruction::IsException {
                    dest: is_exception,
                    value: close_result,
                },
            );
            let continue_block = self.current_function.new_block();
            let exc_block = self.current_function.new_block();
            self.current_function.set_terminator(
                current,
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
                    args: vec![close_result],
                },
            );
            self.emit_propagate_exception_without_iterator_cleanups(exc_block, thrown_val)?;
            current = continue_block;
        }
        Ok(current)
    }

    /// IteratorClose 失败后的 abrupt 传播（不再嵌套 IteratorClose，避免与 emit_throw_value 互递归）。
    fn emit_propagate_exception_without_iterator_cleanups(
        &mut self,
        block: BasicBlockId,
        value: ValueId,
    ) -> Result<StmtFlow, LoweringError> {
        if let Some(try_ctx) = self.try_contexts.last()
            && let Some(catch_entry) = try_ctx.catch_entry
        {
            self.current_function.append_instruction(
                block,
                Instruction::StoreVar {
                    name: try_ctx.exception_var.clone(),
                    value,
                },
            );
            self.current_function.set_terminator(
                block,
                Terminator::Jump {
                    target: catch_entry,
                },
            );
            return Ok(StmtFlow::Terminated);
        }

        let throw_block = self.resolve_store_block(block);
        match self.lower_pending_finalizers(throw_block)? {
            StmtFlow::Open(after_finally) => {
                if self.is_async_generator_fn {
                    let gen_val = self.alloc_value();
                    self.current_function.append_instruction(
                        after_finally,
                        Instruction::LoadVar {
                            dest: gen_val,
                            name: format!("${}.$generator", self.async_generator_scope_id),
                        },
                    );
                    self.current_function.append_instruction(
                        after_finally,
                        Instruction::CallBuiltin {
                            dest: None,
                            builtin: Builtin::AsyncGeneratorThrow,
                            args: vec![gen_val, value],
                        },
                    );
                    self.current_function
                        .set_terminator(after_finally, Terminator::Return { value: None });
                } else if self.is_async_fn {
                    self.emit_async_reject(after_finally, value);
                } else {
                    self.current_function
                        .set_terminator(after_finally, Terminator::Throw { value });
                }
            }
            StmtFlow::Terminated => {}
        }
        Ok(StmtFlow::Terminated)
    }

    /// 正常完成路径上的单次 IteratorClose（completion 为 undefined）。
    pub(crate) fn emit_single_iterator_close_normal(
        &mut self,
        block: BasicBlockId,
        handle: ValueId,
    ) -> Result<BasicBlockId, LoweringError> {
        let block = self.resolve_store_block(block);
        let completion = self.alloc_undefined_value(block);
        self.emit_iterator_closes(block, std::slice::from_ref(&handle), completion)
    }

    // ── labeled ─────────────────────────────────────────────────────────────

    pub(crate) fn lower_labeled(
        &mut self,
        labeled: &swc_ast::LabeledStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        let label_name = labeled.label.sym.to_string();

        if self
            .label_stack
            .iter()
            .any(|ctx| ctx.label.as_deref() == Some(label_name.as_str()))
            || self.pending_loop_label.as_deref() == Some(label_name.as_str())
        {
            return Err(self.error(
                labeled.label.span,
                format!("duplicate label `{label_name}`"),
            ));
        }

        let is_loop_body = matches!(
            labeled.body.as_ref(),
            swc_ast::Stmt::While(_)
                | swc_ast::Stmt::DoWhile(_)
                | swc_ast::Stmt::For(_)
                | swc_ast::Stmt::ForIn(_)
                | swc_ast::Stmt::ForOf(_)
        );

        if is_loop_body {
            let previous = self.pending_loop_label.replace(label_name);
            let inner_flow = self.lower_stmt(&labeled.body, StmtFlow::Open(block));
            self.pending_loop_label = previous;
            return inner_flow;
        }

        let exit = self.current_function.new_block();
        self.label_stack.push(LabelContext {
            label: Some(label_name),
            kind: LabelKind::Block,
            break_target: exit,
            continue_target: None,
            iterator_to_close: None,
        });

        let inner_flow = self.lower_stmt(&labeled.body, StmtFlow::Open(block))?;
        let after = self
            .current_function
            .ensure_jump_or_terminated(inner_flow, exit);

        self.label_stack.pop();
        Ok(after)
    }

    // ── return ──────────────────────────────────────────────────────────────

    pub(crate) fn lower_return(
        &mut self,
        return_stmt: &swc_ast::ReturnStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        let iterator_cleanups = self.active_iterator_cleanups();

        if self.is_async_fn {
            let value = if let Some(arg) = &return_stmt.arg {
                self.lower_expr(arg, block)?
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

            let return_block = self.resolve_store_block(block);
            match self.lower_pending_finalizers(return_block)? {
                StmtFlow::Open(after_finally) => {
                    let after_close = self.emit_iterator_closes(
                        after_finally,
                        &iterator_cleanups,
                        value,
                    )?;
                    if self.is_async_generator_fn {
                        let gen_val = self.alloc_value();
                        self.current_function.append_instruction(
                            after_close,
                            Instruction::LoadVar {
                                dest: gen_val,
                                name: format!("${}.$generator", self.async_generator_scope_id),
                            },
                        );
                        self.current_function.append_instruction(
                            after_close,
                            Instruction::CallBuiltin {
                                dest: None,
                                builtin: Builtin::AsyncGeneratorReturn,
                                args: vec![gen_val, value],
                            },
                        );
                    } else {
                        let promise_val = self.alloc_value();
                        self.current_function.append_instruction(
                            after_close,
                            Instruction::LoadVar {
                                dest: promise_val,
                                name: format!("${}.$promise", self.async_promise_scope_id),
                            },
                        );
                        self.current_function.append_instruction(
                            after_close,
                            Instruction::PromiseResolve {
                                promise: promise_val,
                                value,
                            },
                        );
                    }
                    self.current_function
                        .set_terminator(after_close, Terminator::Return { value: None });
                }
                StmtFlow::Terminated => {}
            }
            return Ok(StmtFlow::Terminated);
        }

        let value = if let Some(arg) = &return_stmt.arg {
            Some(self.lower_expr(arg, block)?)
        } else {
            None
        };

        let return_block = self.resolve_store_block(block);
        match self.lower_pending_finalizers(return_block)? {
            StmtFlow::Open(after_finally) => {
                let completion = match value {
                    Some(v) => v,
                    None => self.alloc_undefined_value(after_finally),
                };
                let after_close =
                    self.emit_iterator_closes(after_finally, &iterator_cleanups, completion)?;
                self.current_function
                    .set_terminator(after_close, Terminator::Return { value });
            }
            StmtFlow::Terminated => {}
        }
        Ok(StmtFlow::Terminated)
    }

    // ── switch ──────────────────────────────────────────────────────────────

    pub(crate) fn lower_switch(
        &mut self,
        switch_stmt: &swc_ast::SwitchStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        let discr = self.lower_expr(&switch_stmt.discriminant, block)?;

        let exit = self.current_function.new_block();
        // 性能优化：预分配容量避免循环中多次 reallocation
        let case_count = switch_stmt.cases.len();
        let mut cases: Vec<SwitchCaseTarget> = Vec::with_capacity(case_count);
        let mut case_blocks: Vec<BasicBlockId> = Vec::with_capacity(case_count);
        let mut default_pos: Option<usize> = None;

        // Generate a case block for each case
        for case in &switch_stmt.cases {
            if case.test.is_none() {
                // default case — 记录其在 cases 中的位置
                default_pos = Some(case_blocks.len());
            }

            let case_block = self.current_function.new_block();
            case_blocks.push(case_block);

            if let Some(test) = &case.test {
                // Compare discriminant with case value
                let _cond_val = self.lower_binary_op_with_const(test, discr, block)?;
                cases.push(SwitchCaseTarget {
                    constant: self.extract_constant_from_expr(test)?,
                    target: case_block,
                });
            }
        }

        // 设置 switch terminator：default 指向 case_blocks[default_pos]，无 default 则分配合成块 jump 到 exit
        let default_target = if let Some(p) = default_pos {
            case_blocks[p]
        } else {
            let synthetic_default = self.current_function.new_block();
            self.current_function
                .set_terminator(synthetic_default, Terminator::Jump { target: exit });
            synthetic_default
        };

        self.current_function.set_terminator(
            block,
            Terminator::Switch {
                value: discr,
                cases,
                default_block: default_target,
                exit_block: exit,
            },
        );

        // Invariant: default_block and exit_block must always be distinct BasicBlockIds.
        // Explicit default → points to case_blocks[default_pos] (allocated before exit).
        // No default → points to synthetic block (allocated before exit, sole terminator Jump { target: exit }).
        // This assertion catches any future regressions where these blocks are aliased.
        debug_assert_ne!(
            default_target, exit,
            "Switch default_block and exit_block must be distinct (default={:?}, exit={:?})",
            default_target, exit
        );

        // Lower case bodies
        self.label_stack.push(LabelContext {
            label: None,
            kind: LabelKind::Switch,
            break_target: exit,
            continue_target: None,
            iterator_to_close: None,
        });

        for (i, case) in switch_stmt.cases.iter().enumerate() {
            let case_block = case_blocks[i];
            let mut case_flow = StmtFlow::Open(case_block);

            for stmt in &case.cons {
                // 严格按照 JavaScript 规范：unreachable code 是合法的，跳过而不报错
                if matches!(case_flow, StmtFlow::Terminated) {
                    continue;
                }
                case_flow = self.lower_stmt(stmt, case_flow)?;
            }

            // Fall-through: if not terminated, jump to next case or exit
            let next_target = if i + 1 < case_blocks.len() {
                case_blocks[i + 1]
            } else {
                exit
            };
            let _ = self
                .current_function
                .ensure_jump_or_terminated(case_flow, next_target);
        }

        // NOTE: default case body 已在上面的 case 循环中一并降低，
        // fallthrough 也由循环中的 ensure_jump_or_terminated 处理，无需单独处理。

        self.label_stack.pop();

        Ok(StmtFlow::Open(exit))
    }

    /// Lower a binary comparison with a constant for switch case matching.
    pub(crate) fn lower_binary_op_with_const(
        &mut self,
        _test: &swc_ast::Expr,
        discr: ValueId,
        _block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // For switch cases, the comparison is implicit StrictEq between discr and case value.
        // This will be handled by the Switch terminator at compile time.
        // We just return the discriminant value for now; the backend handles the comparison.
        Ok(discr)
    }

    pub(crate) fn extract_constant_from_expr(
        &mut self,
        expr: &swc_ast::Expr,
    ) -> Result<ConstantId, LoweringError> {
        match expr {
            swc_ast::Expr::Lit(swc_ast::Lit::Num(num)) => {
                Ok(self.module.add_constant(Constant::Number(num.value)))
            }
            swc_ast::Expr::Lit(swc_ast::Lit::Str(s)) => Ok(self
                .module
                .add_constant(Constant::String(s.value.to_string_lossy().into_owned()))),
            swc_ast::Expr::Lit(swc_ast::Lit::Bool(b)) => {
                Ok(self.module.add_constant(Constant::Bool(b.value)))
            }
            swc_ast::Expr::Lit(swc_ast::Lit::Null(_)) => {
                Ok(self.module.add_constant(Constant::Null))
            }
            swc_ast::Expr::Unary(unary) => match unary.op {
                swc_ast::UnaryOp::Minus => {
                    let swc_ast::Expr::Lit(swc_ast::Lit::Num(num)) = unary.arg.as_ref() else {
                        return Err(self.error(expr.span(), "switch case must be a literal"));
                    };
                    Ok(self.module.add_constant(Constant::Number(-num.value)))
                }
                swc_ast::UnaryOp::Plus => {
                    let swc_ast::Expr::Lit(swc_ast::Lit::Num(num)) = unary.arg.as_ref() else {
                        return Err(self.error(expr.span(), "switch case must be a literal"));
                    };
                    Ok(self.module.add_constant(Constant::Number(num.value)))
                }
                _ => Err(self.error(expr.span(), "switch case must be a literal")),
            },
            _ => Err(self.error(expr.span(), "switch case must be a literal")),
        }
    }

    // ── throw ───────────────────────────────────────────────────────────────

    pub(crate) fn emit_async_reject(&mut self, block: BasicBlockId, reason: ValueId) {
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
            Instruction::PromiseReject {
                promise: promise_val,
                reason,
            },
        );
        self.current_function
            .set_terminator(block, Terminator::Return { value: None });
    }

    pub(crate) fn emit_throw_value(
        &mut self,
        block: BasicBlockId,
        value: ValueId,
    ) -> Result<StmtFlow, LoweringError> {
        if let Some(try_ctx) = self.try_contexts.last()
            && let Some(catch_entry) = try_ctx.catch_entry
        {
            let exc_var = try_ctx.exception_var.clone();
            let iterator_cleanups = self.iterator_cleanups_from_depth(try_ctx.label_depth);
            self.current_function.append_instruction(
                block,
                Instruction::StoreVar {
                    name: exc_var,
                    value,
                },
            );
            let after_close =
                self.emit_iterator_closes(block, &iterator_cleanups, value)?;
            self.current_function.set_terminator(
                after_close,
                Terminator::Jump {
                    target: catch_entry,
                },
            );
            return Ok(StmtFlow::Terminated);
        }

        let throw_block = self.resolve_store_block(block);
        match self.lower_pending_finalizers(throw_block)? {
            StmtFlow::Open(after_finally) => {
                let iterator_cleanups = self.active_iterator_cleanups();
                let after_close = self.emit_iterator_closes(
                    after_finally,
                    &iterator_cleanups,
                    value,
                )?;
                if self.is_async_generator_fn {
                    let gen_val = self.alloc_value();
                    self.current_function.append_instruction(
                        after_close,
                        Instruction::LoadVar {
                            dest: gen_val,
                            name: format!("${}.$generator", self.async_generator_scope_id),
                        },
                    );
                    self.current_function.append_instruction(
                        after_close,
                        Instruction::CallBuiltin {
                            dest: None,
                            builtin: Builtin::AsyncGeneratorThrow,
                            args: vec![gen_val, value],
                        },
                    );
                    self.current_function
                        .set_terminator(after_close, Terminator::Return { value: None });
                } else if self.is_async_fn {
                    self.emit_async_reject(after_close, value);
                } else {
                    self.current_function
                        .set_terminator(after_close, Terminator::Throw { value });
                }
            }
            StmtFlow::Terminated => {}
        }
        Ok(StmtFlow::Terminated)
    }

    pub(crate) fn lower_throw(
        &mut self,
        throw_stmt: &swc_ast::ThrowStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        let value = self.lower_expr(&throw_stmt.arg, block)?;
        self.emit_throw_value(block, value)
    }

    // ── try / catch / finally ───────────────────────────────────────────────

    pub(crate) fn lower_try(
        &mut self,
        try_stmt: &swc_ast::TryStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        // We need to save the current completion state
        // For the initial implementation, we create blocks for the try body,
        // catch body, and finally body, and manage the control flow manually.
        let block = self.ensure_open(flow)?;

        let try_body = self.current_function.new_block();
        let finally_entry = if try_stmt.finalizer.is_some() {
            Some(self.current_function.new_block())
        } else {
            None
        };
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(block, Terminator::Jump { target: try_body });

        // 推入 try context 以便 lower_throw 能重定向到 catch
        let exc_var = self.alloc_temp_name();
        let has_catch = try_stmt.handler.is_some();
        let catch_entry = if has_catch {
            Some(self.current_function.new_block())
        } else {
            None
        };
        let mut try_ctx_popped = false;
        self.try_contexts.push(TryContext {
            catch_entry,
            exception_var: exc_var,
            label_depth: self.label_stack.len(),
        });

        if let Some(finally) = &try_stmt.finalizer {
            self.active_finalizers.push(finally.clone());
        }

        // Lower try body
        let try_flow = self.lower_block_body(&try_stmt.block, StmtFlow::Open(try_body))?;
        if let Some(finally) = &try_stmt.finalizer {
            let finally_entry = finally_entry.expect("finalizer present");
            self.finally_stack.push(FinallyContext {
                _finally_block: finally_entry,
                _after_finally_block: exit,
            });
            let _ = self
                .current_function
                .ensure_jump_or_terminated(try_flow, finally_entry);

            if let Some(catch) = &try_stmt.handler {
                let catch_entry = catch_entry.expect("has_catch implies catch_entry");
                self.scopes.push_scope(ScopeKind::Block);
                if let Some(param) = &catch.param {
                    match param {
                        swc_ast::Pat::Ident(binding) => {
                            let name = binding.id.sym.to_string();
                            let exc_var = self.try_contexts.last().unwrap().exception_var.clone();
                            let exc_val = self.alloc_value();
                            self.current_function.append_instruction(
                                catch_entry,
                                Instruction::LoadVar {
                                    dest: exc_val,
                                    name: exc_var,
                                },
                            );
                            let scope_id = self
                                .scopes
                                .declare(&name, VarKind::Let, true)
                                .map_err(|msg| self.error(param.span(), msg))?;
                            let ir_name = format!("${scope_id}.{name}");
                            self.current_function.append_instruction(
                                catch_entry,
                                Instruction::StoreVar {
                                    name: ir_name,
                                    value: exc_val,
                                },
                            );
                        }
                        _ => {
                            let exc_var = self.try_contexts.last().unwrap().exception_var.clone();
                            let exc_val = self.alloc_value();
                            self.current_function.append_instruction(
                                catch_entry,
                                Instruction::LoadVar {
                                    dest: exc_val,
                                    name: exc_var,
                                },
                            );
                            let mut names = Vec::new();
                            Self::extract_pat_bindings(std::slice::from_ref(param), &mut names);
                            for name in &names {
                                self.scopes
                                    .declare(name, VarKind::Let, true)
                                    .map_err(|msg| self.error(param.span(), msg))?;
                            }
                            self.lower_destructure_pattern(
                                param,
                                exc_val,
                                catch_entry,
                                VarKind::Let,
                            )?;
                        }
                    }
                }
                self.try_contexts.pop();
                try_ctx_popped = true;

                let catch_body_flow =
                    self.lower_block_body(&catch.body, StmtFlow::Open(catch_entry))?;
                let _ = self
                    .current_function
                    .ensure_jump_or_terminated(catch_body_flow, finally_entry);
                self.scopes.pop_scope();
            }

            self.active_finalizers.pop();

            let finally_flow = self.lower_block_body(finally, StmtFlow::Open(finally_entry))?;
            let _ = self
                .current_function
                .ensure_jump_or_terminated(finally_flow, exit);

            self.finally_stack.pop();
        } else if let Some(catch) = &try_stmt.handler {
            let catch_entry = catch_entry.expect("handler present");
            // try/catch without finally
            self.scopes.push_scope(ScopeKind::Block);
            if let Some(param) = &catch.param {
                match param {
                    swc_ast::Pat::Ident(binding) => {
                        let name = binding.id.sym.to_string();
                        let exc_var = self.try_contexts.last().unwrap().exception_var.clone();
                        let exc_val = self.alloc_value();
                        self.current_function.append_instruction(
                            catch_entry,
                            Instruction::LoadVar {
                                dest: exc_val,
                                name: exc_var,
                            },
                        );
                        let scope_id = self
                            .scopes
                            .declare(&name, VarKind::Let, true)
                            .map_err(|msg| self.error(param.span(), msg))?;
                        let ir_name = format!("${scope_id}.{name}");
                        self.current_function.append_instruction(
                            catch_entry,
                            Instruction::StoreVar {
                                name: ir_name,
                                value: exc_val,
                            },
                        );
                    }
                    _ => {
                        let exc_var = self.try_contexts.last().unwrap().exception_var.clone();
                        let exc_val = self.alloc_value();
                        self.current_function.append_instruction(
                            catch_entry,
                            Instruction::LoadVar {
                                dest: exc_val,
                                name: exc_var,
                            },
                        );
                        let mut names = Vec::new();
                        Self::extract_pat_bindings(std::slice::from_ref(param), &mut names);
                        for name in &names {
                            self.scopes
                                .declare(name, VarKind::Let, true)
                                .map_err(|msg| self.error(param.span(), msg))?;
                        }
                        self.lower_destructure_pattern(param, exc_val, catch_entry, VarKind::Let)?;
                    }
                }
            }

            // 进入 catch body 前弹出 try_context，避免自循环
            self.try_contexts.pop();
            try_ctx_popped = true;

            let catch_flow = self.lower_block_body(&catch.body, StmtFlow::Open(catch_entry))?;
            let _ = self
                .current_function
                .ensure_jump_or_terminated(catch_flow, exit);
            self.scopes.pop_scope();

            // Set catch entry as the throw target for the try body
            // If try body throws, it jumps to catch_entry
            // Uncaught throw will terminate. For now, try body that throws jumps to catch_entry.
            let _ = self
                .current_function
                .ensure_jump_or_terminated(try_flow, exit);
        }

        if !try_ctx_popped {
            self.try_contexts.pop();
        }
        Ok(StmtFlow::Open(exit))
    }

    pub(crate) fn lower_block_body(
        &mut self,
        block_stmt: &swc_ast::BlockStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
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

        self.scopes.pop_scope();
        Ok(flow)
    }

    // ── Empty / Debugger / With ─────────────────────────────────────────────

    pub(crate) fn lower_empty(&self, flow: StmtFlow) -> Result<StmtFlow, LoweringError> {
        Ok(flow)
    }
}
