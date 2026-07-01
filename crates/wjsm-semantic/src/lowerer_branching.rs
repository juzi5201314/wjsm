use super::*;

/// abrupt completion 清理序列中的单步：关闭 for-of 迭代器，或运行 try-finally 的 finally 块。
enum UnwindStep {
    IteratorClose(ValueId),
    Finalizer {
        fin_block: swc_ast::BlockStmt,
        fi: usize,
    },
}

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
        if let StmtFlow::Open(after) =
            self.emit_unwind_for_abrupt(block, target_index as isize, None, true)?
        {
            self.current_function
                .set_terminator(after, Terminator::Jump { target });
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
        if let StmtFlow::Open(after) =
            self.emit_unwind_for_abrupt(block, target_index as isize, None, false)?
        {
            self.current_function
                .set_terminator(after, Terminator::Jump { target });
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

    /// 按嵌套深度（内层优先）发射 abrupt completion 的清理序列，交错 finally 块与
    /// IteratorClose。`exit_below`：label_stack 索引 > exit_below 的迭代器、try 的
    /// label_depth > exit_below 的 finalizer 视为正在退出；return/throw 传 -1（全部退出）。
    /// `include_target_iterator`：break 时传 true，将 exit_below 处的循环自身迭代器也
    /// 纳入关闭序列（取代 lower_for_of/for_in 中的 close 中间块）。
    /// 位置 key：迭代器(索引 i) = 2i，finalizer(label_depth d) = 2d-1，二者奇偶不同不会冲突，
    /// 降序排列即得内层优先；同深度 finally 以 finalizer_index 降序（内层 try 先执行）。
    /// `completion`：IteratorClose 的完成值；`None` 时按 break/continue 语义在首个
    /// IteratorClose 处惰性分配 undefined（无迭代器关闭则不产生多余指令）。
    fn emit_unwind_for_abrupt(
        &mut self,
        block: BasicBlockId,
        exit_below: isize,
        completion: Option<ValueId>,
        include_target_iterator: bool,
    ) -> Result<StmtFlow, LoweringError> {
        let mut items: Vec<(i64, i64, UnwindStep)> = Vec::new();
        for (i, ctx) in self.label_stack.iter().enumerate() {
            if ((i as isize) > exit_below
                || (include_target_iterator && (i as isize) == exit_below))
                && let Some(handle) = ctx.iterator_to_close
            {
                items.push(((2 * i) as i64, -1, UnwindStep::IteratorClose(handle)));
            }
        }
        let fin_meta: Vec<(usize, usize)> = self
            .active_finalizers
            .iter()
            .enumerate()
            .filter(|(_, f)| (f.label_depth as isize) > exit_below)
            .map(|(fi, f)| (f.label_depth, fi))
            .collect();
        for (depth, fi) in fin_meta {
            let fin_block = self.active_finalizers[fi].block.clone();
            items.push((
                2 * depth as i64 - 1,
                fi as i64,
                UnwindStep::Finalizer { fin_block, fi },
            ));
        }
        items.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));

        let saved = self.active_finalizers.clone();
        let mut current = block;
        let mut completion = completion;
        for (_, _, step) in items {
            match step {
                UnwindStep::IteratorClose(handle) => {
                    let comp = match completion {
                        Some(v) => v,
                        None => {
                            let v = self.alloc_undefined_value(current);
                            completion = Some(v);
                            v
                        }
                    };
                    current =
                        self.emit_iterator_closes(current, std::slice::from_ref(&handle), comp)?;
                }
                UnwindStep::Finalizer { fin_block, fi } => {
                    // finally 内部的 abrupt completion 只继续展开更外层 finalizer。
                    self.active_finalizers = saved[..fi].to_vec();
                    match self.lower_block_body(&fin_block, StmtFlow::Open(current))? {
                        StmtFlow::Open(after) => current = after,
                        StmtFlow::Terminated => {
                            self.active_finalizers = saved;
                            return Ok(StmtFlow::Terminated);
                        }
                    }
                }
            }
        }
        self.active_finalizers = saved;
        Ok(StmtFlow::Open(current))
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
        let throw_block = self.resolve_store_block(block);
        if self.emit_throw_to_nearest_catch(throw_block, value, false)? {
            return Ok(StmtFlow::Terminated);
        }

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

        if self.is_async_fn {
            let value = if let Some(arg) = &return_stmt.arg {
                self.lower_expr(arg, block)?
            } else {
                self.alloc_undefined_value(block)
            };
            let return_block = self.resolve_store_block(block);
            if let StmtFlow::Open(after_close) =
                self.emit_unwind_for_abrupt(return_block, -1, Some(value), false)?
            {
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
            return Ok(StmtFlow::Terminated);
        }

        let value = if let Some(arg) = &return_stmt.arg {
            Some(self.lower_expr(arg, block)?)
        } else {
            None
        };

        let return_block = self.resolve_store_block(block);
        if let StmtFlow::Open(after_close) =
            self.emit_unwind_for_abrupt(return_block, -1, value, false)?
        {
            self.current_function
                .set_terminator(after_close, Terminator::Return { value });
        }
        Ok(StmtFlow::Terminated)
    }

    // ── switch ──────────────────────────────────────────────────────────────

    /// 降低 switch 语句。
    ///
    /// 按 ECMAScript §14.12.2：每个 case test 是任意表达式，判别式与每个 case test
    /// 用 StrictEq (`===`) 比较，按源码顺序从左到右求值。第一个匹配的 case body
    /// 被执行；若无匹配则跳转到 default；若无 default 则跳到 exit。
    ///
    /// 实现方式：构建测试块链 — 每个 non-default case 对应一个测试块，块内降低
    /// case test 表达式，与判别式做 StrictEq 比较，匹配则跳到 case body，否则
    /// 跳到下一个测试块。最后一个测试块的不匹配分支跳到 default（或 exit）。
    /// case body 的 fall-through 语义保持不变（跳到下一个 case body）。
    pub(crate) fn lower_switch(
        &mut self,
        switch_stmt: &swc_ast::SwitchStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        // ── 降低判别式表达式 ──────────────────────────────────────────────
        let mut discr_block = block;
        let can_throw = self.expr_exception_fork_allowed()
            && self.expr_can_throw(&switch_stmt.discriminant);
        let discr = if can_throw {
            self.lower_expr_then_continue(&switch_stmt.discriminant, &mut discr_block)?
        } else {
            self.lower_expr(&switch_stmt.discriminant, discr_block)?
        };
        // 判别式异常检查（函数调用等可能返回 TAG_EXCEPTION）
        let discr_entry = if can_throw {
            self.lower_value_exception_branch(discr_block, discr)?
        } else {
            self.resolve_store_block(discr_block)
        };

        let exit = self.current_function.new_block();
        let case_count = switch_stmt.cases.len();
        let mut case_blocks: Vec<BasicBlockId> = Vec::with_capacity(case_count);
        let mut default_pos: Option<usize> = None;

        // 为每个 case（含 default）创建 body block，保持源码顺序
        for case in &switch_stmt.cases {
            if case.test.is_none() {
                default_pos = Some(case_blocks.len());
            }
            let case_block = self.current_function.new_block();
            case_blocks.push(case_block);
        }

        // default 目标：有 default → default body block，无 default → exit
        let default_target = if let Some(p) = default_pos {
            case_blocks[p]
        } else {
            exit
        };

        // ── 构建测试块链 ──────────────────────────────────────────────────
        // 为每个 non-default case 预分配测试块
        let test_blocks: Vec<BasicBlockId> = switch_stmt
            .cases
            .iter()
            .filter(|c| c.test.is_some())
            .map(|_| self.current_function.new_block())
            .collect();

        let mut test_idx = 0;
        for (i, case) in switch_stmt.cases.iter().enumerate() {
            let Some(test) = &case.test else {
                continue;
            };

            let test_block = test_blocks[test_idx];
            // 不匹配时跳到下一个测试块，或 default（无更多测试时）
            let next_target = if test_idx + 1 < test_blocks.len() {
                test_blocks[test_idx + 1]
            } else {
                default_target
            };

            // 降低 case test 表达式（任意表达式）
            let mut current_block = test_block;
            let test_can_throw =
                self.expr_exception_fork_allowed() && self.expr_can_throw(test);
            let test_val = if test_can_throw {
                self.lower_expr_then_continue(test, &mut current_block)?
            } else {
                self.lower_expr(test, current_block)?
            };
            // case test 异常检查
            let compare_block = if test_can_throw {
                self.lower_value_exception_branch(current_block, test_val)?
            } else {
                self.resolve_store_block(current_block)
            };

            // StrictEq 比较：discr === test_val
            let cmp_dest = self.alloc_value();
            self.current_function.append_instruction(
                compare_block,
                Instruction::Compare {
                    dest: cmp_dest,
                    op: CompareOp::StrictEq,
                    lhs: discr,
                    rhs: test_val,
                },
            );

            // 匹配 → case body，不匹配 → 下一个测试块或 default
            self.current_function.set_terminator(
                compare_block,
                Terminator::Branch {
                    condition: cmp_dest,
                    true_block: case_blocks[i],
                    false_block: next_target,
                },
            );

            test_idx += 1;
        }

        // 入口块跳到第一个测试块（或 default/exit，若无测试块）
        let entry_target = test_blocks.first().copied().unwrap_or(default_target);
        self.current_function
            .set_terminator(discr_entry, Terminator::Jump { target: entry_target });

        // ── 降低 case body（含 fall-through 和 break）────────────────────
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

            // Fall-through: if not terminated, jump to next case body or exit
            let next_target = if i + 1 < case_blocks.len() {
                case_blocks[i + 1]
            } else {
                exit
            };
            let _ = self
                .current_function
                .ensure_jump_or_terminated(case_flow, next_target);
        }

        self.label_stack.pop();
        Ok(StmtFlow::Open(exit))
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

    fn nearest_catch_context(&self) -> Option<(usize, BasicBlockId, String, usize)> {
        self.try_contexts
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, ctx)| {
                ctx.catch_entry.map(|catch_entry| {
                    (
                        index,
                        catch_entry,
                        ctx.exception_var.clone(),
                        ctx.label_depth,
                    )
                })
            })
    }

    fn finalizer_keep_len_for_try_context(&self, target_index: usize) -> usize {
        self.try_contexts[..=target_index]
            .iter()
            .filter_map(|ctx| ctx.finalizer_index.map(|index| index + 1))
            .max()
            .unwrap_or(0)
    }

    fn emit_throw_to_nearest_catch(
        &mut self,
        block: BasicBlockId,
        value: ValueId,
        close_iterators: bool,
    ) -> Result<bool, LoweringError> {
        let Some((target_index, catch_entry, exc_var, label_depth)) = self.nearest_catch_context()
        else {
            return Ok(false);
        };
        let keep_len = self.finalizer_keep_len_for_try_context(target_index);
        match self.lower_pending_finalizers_after(block, keep_len)? {
            StmtFlow::Open(after_finally) => {
                self.current_function.append_instruction(
                    after_finally,
                    Instruction::StoreVar {
                        name: exc_var,
                        value,
                    },
                );
                let target_block = if close_iterators {
                    let iterator_cleanups = self.iterator_cleanups_from_depth(label_depth);
                    self.emit_iterator_closes(after_finally, &iterator_cleanups, value)?
                } else {
                    after_finally
                };
                self.current_function.set_terminator(
                    target_block,
                    Terminator::Jump {
                        target: catch_entry,
                    },
                );
            }
            StmtFlow::Terminated => {}
        }
        Ok(true)
    }

    pub(crate) fn emit_throw_value(
        &mut self,
        block: BasicBlockId,
        value: ValueId,
    ) -> Result<StmtFlow, LoweringError> {
        let throw_block = self.resolve_store_block(block);
        if self.emit_throw_to_nearest_catch(throw_block, value, true)? {
            return Ok(StmtFlow::Terminated);
        }

        match self.lower_pending_finalizers(throw_block)? {
            StmtFlow::Open(after_finally) => {
                let iterator_cleanups = self.active_iterator_cleanups();
                let after_close =
                    self.emit_iterator_closes(after_finally, &iterator_cleanups, value)?;
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
            finalizer_index: try_stmt
                .finalizer
                .as_ref()
                .map(|_| self.active_finalizers.len()),
        });

        if let Some(finally) = &try_stmt.finalizer {
            self.active_finalizers.push(PendingFinalizer {
                block: finally.clone(),
                label_depth: self.label_stack.len(),
            });
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
                            self.try_contexts.pop();
                            try_ctx_popped = true;
                            let exc_val = self.alloc_value();
                            self.current_function.append_instruction(
                                catch_entry,
                                Instruction::LoadVar {
                                    dest: exc_val,
                                    name: exc_var,
                                },
                            );
                            // 检查 null/undefined 并抛出 TypeError
                            let is_nullish = self.alloc_value();
                            self.current_function.append_instruction(
                                catch_entry,
                                Instruction::Unary {
                                    dest: is_nullish,
                                    op: UnaryOp::IsNullish,
                                    value: exc_val,
                                },
                            );
                            let destructure_block = self.current_function.new_block();
                            let throw_block = self.current_function.new_block();
                            self.current_function.set_terminator(
                                catch_entry,
                                Terminator::Branch {
                                    condition: is_nullish,
                                    true_block: throw_block,
                                    false_block: destructure_block,
                                },
                            );
                            {
                                let msg_const = self.module.add_constant(Constant::String(
                                    "Cannot destructure null or undefined".to_string(),
                                ));
                                let msg_val = self.alloc_value();
                                self.current_function.append_instruction(
                                    throw_block,
                                    Instruction::Const {
                                        dest: msg_val,
                                        constant: msg_const,
                                    },
                                );
                                let error_val = self.alloc_value();
                                self.current_function.append_instruction(
                                    throw_block,
                                    Instruction::CallBuiltin {
                                        dest: Some(error_val),
                                        builtin: Builtin::TypeErrorConstructor,
                                        args: vec![msg_val],
                                    },
                                );
                                self.emit_throw_value(throw_block, error_val)?;
                            }
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
                                destructure_block,
                                VarKind::Let,
                            )?;
                        }
                    }
                }
                if !try_ctx_popped {
                    self.try_contexts.pop();
                }
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
            let mut catch_entry = catch_entry.expect("handler present");
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
                        // 先取出 exc_var，再弹出 try_context
                        let exc_var = self.try_contexts.last().unwrap().exception_var.clone();
                        self.try_contexts.pop();
                        try_ctx_popped = true;
                        let exc_val = self.alloc_value();
                        self.current_function.append_instruction(
                            catch_entry,
                            Instruction::LoadVar {
                                dest: exc_val,
                                name: exc_var,
                            },
                        );
                        // 检查 exc_val 是否为 null/undefined；
                        // 若是，抛出 TypeError（ECMAScript §8.5.5 / destructuring binding pattern）
                        let is_nullish = self.alloc_value();
                        self.current_function.append_instruction(
                            catch_entry,
                            Instruction::Unary {
                                dest: is_nullish,
                                op: UnaryOp::IsNullish,
                                value: exc_val,
                            },
                        );
                        let destructure_block = self.current_function.new_block();
                        let throw_block = self.current_function.new_block();
                        self.current_function.set_terminator(
                            catch_entry,
                            Terminator::Branch {
                                condition: is_nullish,
                                true_block: throw_block,
                                false_block: destructure_block,
                            },
                        );
                        // throw 分支：构造 TypeError 并抛出
                        {
                            let msg_const = self.module.add_constant(Constant::String(
                                "Cannot destructure null or undefined".to_string(),
                            ));
                            let msg_val = self.alloc_value();
                            self.current_function.append_instruction(
                                throw_block,
                                Instruction::Const {
                                    dest: msg_val,
                                    constant: msg_const,
                                },
                            );
                            let error_val = self.alloc_value();
                            self.current_function.append_instruction(
                                throw_block,
                                Instruction::CallBuiltin {
                                    dest: Some(error_val),
                                    builtin: Builtin::TypeErrorConstructor,
                                    args: vec![msg_val],
                                },
                            );
                            self.emit_throw_value(throw_block, error_val)?;
                        }
                        let mut names = Vec::new();
                        Self::extract_pat_bindings(std::slice::from_ref(param), &mut names);
                        for name in &names {
                            self.scopes
                                .declare(name, VarKind::Let, true)
                                .map_err(|msg| self.error(param.span(), msg))?;
                        }
                        let after_destruct = self.lower_destructure_pattern(
                            param,
                            exc_val,
                            destructure_block,
                            VarKind::Let,
                        )?;
                        catch_entry = after_destruct;
                    }
                }
            }

            // 进入 catch body 前弹出 try_context，避免自循环
            if !try_ctx_popped {
                self.try_contexts.pop();
                try_ctx_popped = true;
            }

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
