//! `yield*` 委托的 lowering（§27.5.3.7 YieldExpression : yield * AssignmentExpression）。
//!
//! sync 形态围绕宿主 `array_iterators` 条目组织 header/body/exit 三段循环：
//! header 经 IteratorDone 惰性推进、body 取值后挂起、exit 以最终 result.value
//! 作为委托表达式的值。async 形态每轮 Await inner.next() 后按 done 分支。
//! 两种形态的委托体内 yield 都用本模块的专用挂起点：resume completion 为
//! throw/return 时经宿主 IteratorDelegate*/AsyncIteratorDelegate* 向内层
//! 迭代器转发（步骤 7.b/7.c），而不是像普通 yield 那样直接在外层展开。

use super::*;

impl Lowerer {
    /// 声明委托迭代器的临时绑定并把迭代器句柄存入：跨 yield/await 挂起的
    /// 迭代器必须落在作用域变量上，由续体 save/restore 机制保活。
    fn declare_yield_star_iter(
        &mut self,
        block: BasicBlockId,
        iter_handle: ValueId,
        span: swc_core::common::Span,
    ) -> Result<String, LoweringError> {
        let iter_name = format!("$yield_star_iter_{}", self.next_temp);
        self.next_temp += 1;
        let scope_id = self
            .scopes
            .declare(&iter_name, VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let iter_ir_name = format!("${scope_id}.{iter_name}");
        self.current_function.append_instruction(
            block,
            Instruction::StoreVar {
                name: iter_ir_name.clone(),
                value: iter_handle,
            },
        );
        Ok(iter_ir_name)
    }

    pub(crate) fn lower_sync_yield_delegate(
        &mut self,
        iterable: ValueId,
        block: BasicBlockId,
        span: swc_core::common::Span,
    ) -> Result<ValueId, LoweringError> {
        let iter_handle = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(iter_handle),
                builtin: Builtin::IteratorFrom,
                args: vec![iterable],
            },
        );
        // GetIterator（§7.4.3）必抛：nullish / 无 @@iterator 源产生的 TypeError
        // 哨兵不得作为迭代器流入委托循环（否则 `yield* null` 静默完成）。
        let block = self.lower_value_exception_branch(block, iter_handle)?;
        let iter_ir_name = self.declare_yield_star_iter(block, iter_handle, span)?;

        let header = self.current_function.new_block();
        let body = self.current_function.new_block();
        let exit = self.current_function.new_block();
        self.current_function
            .set_terminator(block, Terminator::Jump { target: header });

        let iter_for_done = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::LoadVar {
                dest: iter_for_done,
                name: iter_ir_name.clone(),
            },
        );
        let done = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::CallBuiltin {
                dest: Some(done),
                builtin: Builtin::IteratorDone,
                args: vec![iter_for_done],
            },
        );
        // §27.5.3.7 步骤 7.a.i `? IteratorNext`：内部迭代器 next() 的抛出经
        // IteratorDone 浮出（宿主在 done 探测时惰性推进），必须分叉传播。
        let condition_block = self.lower_value_exception_branch(header, done)?;
        let not_done = self.alloc_value();
        self.current_function.append_instruction(
            condition_block,
            Instruction::Unary {
                dest: not_done,
                op: UnaryOp::Not,
                value: done,
            },
        );
        self.current_function.set_terminator(
            condition_block,
            Terminator::Branch {
                condition: not_done,
                true_block: body,
                false_block: exit,
            },
        );

        let iter_for_value = self.alloc_value();
        self.current_function.append_instruction(
            body,
            Instruction::LoadVar {
                dest: iter_for_value,
                name: iter_ir_name.clone(),
            },
        );
        let yielded_value = self.alloc_value();
        self.current_function.append_instruction(
            body,
            Instruction::CallBuiltin {
                dest: Some(yielded_value),
                builtin: Builtin::IteratorValue,
                args: vec![iter_for_value],
            },
        );
        // §27.5.3.7 步骤 7.a.iv `? IteratorValue`：result.value getter 抛出
        // 不得作为普通值 yield 出去。
        let body_open = self.lower_value_exception_branch(body, yielded_value)?;
        let after_yield =
            self.emit_sync_delegate_yield(body_open, yielded_value, &iter_ir_name, header)?;
        let iter_for_next = self.alloc_value();
        self.current_function.append_instruction(
            after_yield,
            Instruction::LoadVar {
                dest: iter_for_next,
                name: iter_ir_name.clone(),
            },
        );
        self.current_function.append_instruction(
            after_yield,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::IteratorNext,
                args: vec![iter_for_next],
            },
        );
        self.current_function
            .set_terminator(after_yield, Terminator::Jump { target: header });

        let iter_for_final = self.alloc_value();
        self.current_function.append_instruction(
            exit,
            Instruction::LoadVar {
                dest: iter_for_final,
                name: iter_ir_name,
            },
        );
        let final_value = self.alloc_value();
        self.current_function.append_instruction(
            exit,
            Instruction::CallBuiltin {
                dest: Some(final_value),
                builtin: Builtin::IteratorValue,
                args: vec![iter_for_final],
            },
        );
        // §27.5.3.7 步骤 7.a.iii：最终 result.value 的读取抛出同样分叉传播，
        // 不得作为委托表达式的值流入后续代码。
        let exit = self.lower_value_exception_branch(exit, final_value)?;

        self.expr_merge_block = Some(exit);
        Ok(final_value)
    }

    /// async generator 的 `yield*`（§27.5.3.7 的 async 形态）：
    /// GetIterator(value, async)（§7.4.3）必抛先于委托循环；每轮 Await
    /// inner.next() 的结果后按 result.done 分支——未完成则
    /// AsyncGeneratorYield(result.value) 挂起，完成则以最终 result.value
    /// 作为委托表达式的值（步骤 7.a.iii）。
    pub(crate) fn lower_async_yield_delegate(
        &mut self,
        iterable: ValueId,
        block: BasicBlockId,
        span: swc_core::common::Span,
    ) -> Result<ValueId, LoweringError> {
        let iter_handle = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(iter_handle),
                builtin: Builtin::AsyncIteratorFrom,
                args: vec![iterable],
            },
        );
        // GetIterator（§7.4.3）必抛：nullish / 非可迭代源产生的 TypeError
        // 哨兵不得作为迭代器流入委托循环（否则 `yield* null` 静默完成）。
        let block = self.lower_value_exception_branch(block, iter_handle)?;
        let iter_ir_name = self.declare_yield_star_iter(block, iter_handle, span)?;

        let header = self.current_function.new_block();
        let body = self.current_function.new_block();
        let exit = self.current_function.new_block();
        self.current_function
            .set_terminator(block, Terminator::Jump { target: header });

        let awaited_result = self.emit_delegate_next_await(header, &iter_ir_name)?;
        let after_await = self.resolve_store_block(header);
        let done_val = self.emit_get_string_prop(after_await, awaited_result, "done");
        let not_done = self.alloc_value();
        self.current_function.append_instruction(
            after_await,
            Instruction::Unary {
                dest: not_done,
                op: UnaryOp::Not,
                value: done_val,
            },
        );
        self.current_function.set_terminator(
            after_await,
            Terminator::Branch {
                condition: not_done,
                true_block: body,
                false_block: exit,
            },
        );

        // body：AsyncGeneratorYield(result.value) 挂起；resume completion
        // 为 throw/return 时向内转发，normal 则回 header 续走下一轮。
        let result_for_value = self.emit_load_resume_val(body);
        let yielded_value = self.emit_get_string_prop(body, result_for_value, "value");
        let after_yield =
            self.emit_async_delegate_yield(body, yielded_value, &iter_ir_name, body, exit)?;
        self.current_function
            .set_terminator(after_yield, Terminator::Jump { target: header });

        // exit：委托表达式的值为最终 result.value（§27.5.3.7 步骤 7.a.iii；
        // received throw 转发得到 done 结果时同为 normal 完成，步骤 7.b.ii.6）。
        let final_result = self.emit_load_resume_val(exit);
        let final_value = self.emit_get_string_prop(exit, final_result, "value");
        self.expr_merge_block = Some(exit);
        Ok(final_value)
    }

    /// 在 `header` 里发射 `Await(inner.next())`：next 结果经 Promise.resolve
    /// 包装后挂起，resume 时 rejection 走 throw 路径（含 next 调用本身返回
    /// 的异常哨兵——PromiseResolveStatic 把哨兵转为 rejected promise）。
    /// 返回 await 后的迭代结果值（从 `$resume_val` 加载）。
    fn emit_delegate_next_await(
        &mut self,
        header: BasicBlockId,
        iter_ir_name: &str,
    ) -> Result<ValueId, LoweringError> {
        let iter_for_next = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::LoadVar {
                dest: iter_for_next,
                name: iter_ir_name.to_string(),
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
        let continue_block = self.emit_delegate_await(header, next_call_result, None)?;
        let awaited_result = self.emit_load_resume_val(continue_block);
        self.expr_merge_block = Some(continue_block);
        Ok(awaited_result)
    }

    /// 发射一次委托内部的 Await：`value` 经 Promise.resolve 包装后挂起。
    /// rejection 默认走 throw 路径（emit_throw_value 在外层生成器展开），
    /// `reject_to` 提供时改跳指定块（AsyncGeneratorUnwrapYieldResumption
    /// 步骤 3 把 return 值的拒绝转成 throw completion 继续委托转发）。
    /// 返回 resolve 侧的延续块（awaited 值在 `$resume_val`）。
    fn emit_delegate_await(
        &mut self,
        block: BasicBlockId,
        value: ValueId,
        reject_to: Option<BasicBlockId>,
    ) -> Result<BasicBlockId, LoweringError> {
        let undef_const = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: undef_val,
                constant: undef_const,
            },
        );
        let promised = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(promised),
                builtin: Builtin::PromiseResolveStatic,
                args: vec![undef_val, value],
            },
        );

        let next_state = self.async_state_counter;
        self.async_state_counter += 1;
        let resume_block = self.current_function.new_block();
        let continue_block = self.current_function.new_block();
        self.async_resume_blocks.push((next_state, resume_block));
        let visible_bindings = self.async_visible_binding_names();
        self.pending_suspends.push(PendingSuspend {
            suspend_block: block,
            resume_block,
            visible_bindings,
        });
        self.current_function.append_instruction(
            block,
            Instruction::Suspend {
                promise: promised,
                state: next_state,
            },
        );
        self.current_function.set_terminator(
            block,
            Terminator::Jump {
                target: continue_block,
            },
        );

        let resume_val = self.emit_load_resume_val(resume_block);
        let is_rejected = self.alloc_value();
        self.current_function.append_instruction(
            resume_block,
            Instruction::LoadVar {
                dest: is_rejected,
                name: format!("${}.$is_rejected", self.async_is_rejected_scope_id),
            },
        );
        match reject_to {
            Some(target) => {
                self.current_function.set_terminator(
                    resume_block,
                    Terminator::Branch {
                        condition: is_rejected,
                        true_block: target,
                        false_block: continue_block,
                    },
                );
            }
            None => {
                let throw_block = self.current_function.new_block();
                self.current_function.set_terminator(
                    resume_block,
                    Terminator::Branch {
                        condition: is_rejected,
                        true_block: throw_block,
                        false_block: continue_block,
                    },
                );
                self.emit_throw_value(throw_block, resume_val)?;
            }
        }
        Ok(continue_block)
    }

    /// 加载当前 resume 值（await / yield 恢复后由宿主写入的 `$resume_val`）。
    fn emit_load_resume_val(&mut self, block: BasicBlockId) -> ValueId {
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest,
                name: format!("${}.$resume_val", self.async_resume_val_scope_id),
            },
        );
        dest
    }

    /// 以字符串常量键发射一次普通 GetProp。
    fn emit_get_string_prop(
        &mut self,
        block: BasicBlockId,
        object: ValueId,
        key: &str,
    ) -> ValueId {
        let key_const = self.module.add_constant(Constant::String(key.to_string()));
        let key_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: key_val,
                constant: key_const,
            },
        );
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::GetProp {
                dest,
                object,
                key: key_val,
            },
        );
        dest
    }

    /// 加载协程的 `$generator` 句柄。
    fn emit_load_generator(&mut self, block: BasicBlockId) -> ValueId {
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest,
                name: format!("${}.$generator", self.async_generator_scope_id),
            },
        );
        dest
    }

    /// 加载 resume completion 类型（宿主写入的 `$is_rejected`：0 normal、
    /// 1 throw、2 return）。
    fn emit_load_completion_kind(&mut self, block: BasicBlockId) -> ValueId {
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest,
                name: format!("${}.$is_rejected", self.async_is_rejected_scope_id),
            },
        );
        dest
    }

    /// 发射 `value === <number>` 比较（completion 类型 / 标记 k 的分派键）。
    fn emit_eq_number(&mut self, block: BasicBlockId, value: ValueId, number: f64) -> ValueId {
        let number_const = self.module.add_constant(Constant::Number(number));
        let number_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: number_val,
                constant: number_const,
            },
        );
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Compare {
                dest,
                op: CompareOp::StrictEq,
                lhs: value,
                rhs: number_val,
            },
        );
        dest
    }

    /// 委托转发得到的 ReturnCompletion（§27.5.3.7 步骤 7.c.iii / 7.c.viii）：
    /// 与普通 yield 恢复为 return completion 的路径同构——迭代器保护区
    /// （ForIn/OfBodyEvaluation 与解构 §8.6.2 的 IteratorClose）与 finally
    /// 按嵌套深度内层优先交错展开后，把完成值交给宿主的
    /// GeneratorReturn / AsyncGeneratorReturn 收尾。
    fn emit_delegate_return_completion(
        &mut self,
        block: BasicBlockId,
        value: ValueId,
    ) -> Result<(), LoweringError> {
        let return_slot = self.preserve_suspending_completion(block, value);
        match self.emit_unwind_for_abrupt(block, -1, Some(value), false, return_slot.as_deref())? {
            StmtFlow::Open(after_unwind) => {
                let gen_val = self.emit_load_generator(after_unwind);
                let return_value =
                    self.reload_suspending_completion(after_unwind, value, return_slot.as_deref());
                if self.is_async_generator_fn {
                    self.current_function.append_instruction(
                        after_unwind,
                        Instruction::CallBuiltin {
                            dest: None,
                            builtin: Builtin::AsyncGeneratorReturn,
                            args: vec![gen_val, return_value],
                        },
                    );
                    self.current_function
                        .set_terminator(after_unwind, Terminator::Return { value: None });
                } else {
                    let final_result = self.alloc_value();
                    self.current_function.append_instruction(
                        after_unwind,
                        Instruction::CallBuiltin {
                            dest: Some(final_result),
                            builtin: Builtin::GeneratorReturn,
                            args: vec![gen_val, return_value],
                        },
                    );
                    self.current_function.set_terminator(
                        after_unwind,
                        Terminator::Return {
                            value: Some(final_result),
                        },
                    );
                }
            }
            StmtFlow::Terminated => {}
        }
        Ok(())
    }

    /// sync 委托体内的 yield 挂起点（步骤 7.a.vii）：resume completion 分派
    /// ——throw（1）经宿主 IteratorDelegateThrow 向内转发后回 header（结果
    /// 已缓存进迭代器条目，done 分支由 header 续走）；return（2）经
    /// IteratorDelegateReturn 转发，方法缺失或结果 done 则 ReturnCompletion，
    /// 未完成回 header 继续委托；normal（0）落延续块由调用方推进 next。
    fn emit_sync_delegate_yield(
        &mut self,
        block: BasicBlockId,
        value: ValueId,
        iter_ir_name: &str,
        header: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        let gen_val = self.emit_load_generator(block);
        let next_state = self.async_state_counter;
        self.async_state_counter += 1;
        let resume_block = self.current_function.new_block();
        let continue_block = self.current_function.new_block();
        self.async_resume_blocks.push((next_state, resume_block));
        let visible_bindings = self.async_visible_binding_names();
        self.pending_suspends.push(PendingSuspend {
            suspend_block: block,
            resume_block,
            visible_bindings,
        });
        let result = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(result),
                builtin: Builtin::GeneratorNext,
                args: vec![gen_val, value],
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::GeneratorSuspend {
                result,
                state: next_state,
            },
        );
        self.current_function.set_terminator(
            block,
            Terminator::Jump {
                target: continue_block,
            },
        );

        let completion = self.emit_load_completion_kind(resume_block);
        let throw_forward = self.current_function.new_block();
        let return_forward = self.current_function.new_block();
        let check_return = self.current_function.new_block();
        let is_throw = self.emit_eq_number(resume_block, completion, 1.0);
        self.current_function.set_terminator(
            resume_block,
            Terminator::Branch {
                condition: is_throw,
                true_block: throw_forward,
                false_block: check_return,
            },
        );
        let is_return = self.emit_eq_number(check_return, completion, 2.0);
        self.current_function.set_terminator(
            check_return,
            Terminator::Branch {
                condition: is_return,
                true_block: return_forward,
                false_block: continue_block,
            },
        );

        // received throw（步骤 7.b）：转发结果已缓存进条目，回 header 按
        // done 续走——done 为 normal 完成（exit 读最终 value，步骤 7.b.ii.6），
        // 未完成继续 yield（步骤 7.b.ii.7）；缺 throw 方法的 close+TypeError
        // 与各段抛出经异常分叉在外层生成器展开。
        {
            let received = self.emit_load_resume_val(throw_forward);
            let iter = self.alloc_value();
            self.current_function.append_instruction(
                throw_forward,
                Instruction::LoadVar {
                    dest: iter,
                    name: iter_ir_name.to_string(),
                },
            );
            let forwarded = self.alloc_value();
            self.current_function.append_instruction(
                throw_forward,
                Instruction::CallBuiltin {
                    dest: Some(forwarded),
                    builtin: Builtin::IteratorDelegateThrow,
                    args: vec![iter, received],
                },
            );
            let after = self.lower_value_exception_branch(throw_forward, forwarded)?;
            self.current_function
                .set_terminator(after, Terminator::Jump { target: header });
        }

        // received return（步骤 7.c）：undefined 哨兵为方法缺失——
        // ReturnCompletion(received)（步骤 7.c.iii）；否则按条目 done 分支：
        // done 则 ReturnCompletion(? IteratorValue)（步骤 7.c.viii），未完成
        // 回 header 继续委托（步骤 7.c.x）。
        {
            let received = self.emit_load_resume_val(return_forward);
            let iter = self.alloc_value();
            self.current_function.append_instruction(
                return_forward,
                Instruction::LoadVar {
                    dest: iter,
                    name: iter_ir_name.to_string(),
                },
            );
            let forwarded = self.alloc_value();
            self.current_function.append_instruction(
                return_forward,
                Instruction::CallBuiltin {
                    dest: Some(forwarded),
                    builtin: Builtin::IteratorDelegateReturn,
                    args: vec![iter, received],
                },
            );
            let after = self.lower_value_exception_branch(return_forward, forwarded)?;
            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                after,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );
            let is_missing = self.alloc_value();
            self.current_function.append_instruction(
                after,
                Instruction::Compare {
                    dest: is_missing,
                    op: CompareOp::StrictEq,
                    lhs: forwarded,
                    rhs: undef_val,
                },
            );
            let return_received = self.current_function.new_block();
            let check_done = self.current_function.new_block();
            self.current_function.set_terminator(
                after,
                Terminator::Branch {
                    condition: is_missing,
                    true_block: return_received,
                    false_block: check_done,
                },
            );
            self.emit_delegate_return_completion(return_received, received)?;

            let iter_for_done = self.alloc_value();
            self.current_function.append_instruction(
                check_done,
                Instruction::LoadVar {
                    dest: iter_for_done,
                    name: iter_ir_name.to_string(),
                },
            );
            // 条目状态刚由宿主写入（无 JS 可观察副作用），直接读缓存分支。
            let done = self.alloc_value();
            self.current_function.append_instruction(
                check_done,
                Instruction::CallBuiltin {
                    dest: Some(done),
                    builtin: Builtin::IteratorDone,
                    args: vec![iter_for_done],
                },
            );
            let return_done = self.current_function.new_block();
            self.current_function.set_terminator(
                check_done,
                Terminator::Branch {
                    condition: done,
                    true_block: return_done,
                    false_block: header,
                },
            );
            let iter_for_final = self.alloc_value();
            self.current_function.append_instruction(
                return_done,
                Instruction::LoadVar {
                    dest: iter_for_final,
                    name: iter_ir_name.to_string(),
                },
            );
            let final_value = self.alloc_value();
            self.current_function.append_instruction(
                return_done,
                Instruction::CallBuiltin {
                    dest: Some(final_value),
                    builtin: Builtin::IteratorValue,
                    args: vec![iter_for_final],
                },
            );
            // 步骤 7.c.viii.1 `? IteratorValue`：value getter 抛出传播。
            let after_final = self.lower_value_exception_branch(return_done, final_value)?;
            self.emit_delegate_return_completion(after_final, final_value)?;
        }

        Ok(continue_block)
    }

    /// async 委托体内的 yield 挂起点（步骤 7.a.vi）：resume completion 分派
    /// ——throw（1）经 AsyncIteratorDelegateThrow 转发，标记 k=0 为方法调用
    /// 结果（Await 后对象校验、按 done 分支到 body/exit）、k=1 为缺方法时
    /// close 结果（Await + 对象校验后抛缺方法 TypeError）；return（2）先
    /// Await(received)（AsyncGeneratorUnwrapYieldResumption），拒绝转 throw
    /// 转发，成功经 AsyncIteratorDelegateReturn 转发（k=2 方法缺失 →
    /// Await 后 ReturnCompletion；k=0 → Await 后按 done 分支）；normal（0）
    /// 落延续块由调用方跳回 header。
    fn emit_async_delegate_yield(
        &mut self,
        block: BasicBlockId,
        value: ValueId,
        iter_ir_name: &str,
        body: BasicBlockId,
        exit: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        let gen_val = self.emit_load_generator(block);
        let next_state = self.async_state_counter;
        self.async_state_counter += 1;
        let resume_block = self.current_function.new_block();
        let continue_block = self.current_function.new_block();
        self.async_resume_blocks.push((next_state, resume_block));
        let visible_bindings = self.async_visible_binding_names();
        self.pending_suspends.push(PendingSuspend {
            suspend_block: block,
            resume_block,
            visible_bindings,
        });
        let promised = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(promised),
                builtin: Builtin::AsyncGeneratorNext,
                args: vec![gen_val, value],
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::Suspend {
                promise: promised,
                state: next_state,
            },
        );
        self.current_function.set_terminator(
            block,
            Terminator::Jump {
                target: continue_block,
            },
        );

        let completion = self.emit_load_completion_kind(resume_block);
        let throw_forward = self.current_function.new_block();
        let return_unwrap = self.current_function.new_block();
        let check_return = self.current_function.new_block();
        let is_throw = self.emit_eq_number(resume_block, completion, 1.0);
        self.current_function.set_terminator(
            resume_block,
            Terminator::Branch {
                condition: is_throw,
                true_block: throw_forward,
                false_block: check_return,
            },
        );
        let is_return = self.emit_eq_number(check_return, completion, 2.0);
        self.current_function.set_terminator(
            check_return,
            Terminator::Branch {
                condition: is_return,
                true_block: return_unwrap,
                false_block: continue_block,
            },
        );

        // received throw（步骤 7.b 的 async 形态）。
        {
            let received = self.emit_load_resume_val(throw_forward);
            let iter = self.alloc_value();
            self.current_function.append_instruction(
                throw_forward,
                Instruction::LoadVar {
                    dest: iter,
                    name: iter_ir_name.to_string(),
                },
            );
            let marker = self.alloc_value();
            self.current_function.append_instruction(
                throw_forward,
                Instruction::CallBuiltin {
                    dest: Some(marker),
                    builtin: Builtin::AsyncIteratorDelegateThrow,
                    args: vec![iter, received],
                },
            );
            let after = self.lower_value_exception_branch(throw_forward, marker)?;
            let kind = self.emit_get_string_prop(after, marker, "k");
            let payload = self.emit_get_string_prop(after, marker, "v");
            let is_call_result = self.emit_eq_number(after, kind, 0.0);
            let await_call = self.current_function.new_block();
            let await_close = self.current_function.new_block();
            self.current_function.set_terminator(
                after,
                Terminator::Branch {
                    condition: is_call_result,
                    true_block: await_call,
                    false_block: await_close,
                },
            );

            // k=0：Await(innerResult)（步骤 7.b.ii.2）→ 对象校验（步骤
            // 7.b.ii.4）→ done 分支（未完成续 yield，done 为 normal 完成）。
            let resolved = self.emit_delegate_await(await_call, payload, None)?;
            let inner_result = self.emit_load_resume_val(resolved);
            let checked = self.alloc_value();
            self.current_function.append_instruction(
                resolved,
                Instruction::CallBuiltin {
                    dest: Some(checked),
                    builtin: Builtin::IteratorResultRequireObject,
                    args: vec![inner_result],
                },
            );
            let resolved = self.lower_value_exception_branch(resolved, checked)?;
            let done = self.emit_get_string_prop(resolved, checked, "done");
            // 步骤 7.b.ii.5 `? IteratorComplete`：done getter 抛出传播。
            let resolved = self.lower_value_exception_branch(resolved, done)?;
            let not_done = self.alloc_value();
            self.current_function.append_instruction(
                resolved,
                Instruction::Unary {
                    dest: not_done,
                    op: UnaryOp::Not,
                    value: done,
                },
            );
            self.current_function.set_terminator(
                resolved,
                Terminator::Branch {
                    condition: not_done,
                    true_block: body,
                    false_block: exit,
                },
            );

            // k=1：Await close 结果（§7.4.10 步骤 5，拒绝胜出）→ 对象校验
            // （步骤 6）→ 抛缺方法 TypeError（步骤 7.b.iii.6）。
            let resolved = self.emit_delegate_await(await_close, payload, None)?;
            let close_result = self.emit_load_resume_val(resolved);
            let close_checked = self.alloc_value();
            self.current_function.append_instruction(
                resolved,
                Instruction::CallBuiltin {
                    dest: Some(close_checked),
                    builtin: Builtin::IteratorResultRequireObject,
                    args: vec![close_result],
                },
            );
            let resolved = self.lower_value_exception_branch(resolved, close_checked)?;
            let missing_error = self.alloc_value();
            self.current_function.append_instruction(
                resolved,
                Instruction::CallBuiltin {
                    dest: Some(missing_error),
                    builtin: Builtin::IteratorThrowMethodMissingError,
                    args: vec![],
                },
            );
            self.emit_throw_value(resolved, missing_error)?;
        }

        // received return：先 Await(received)（AsyncGeneratorUnwrapYieldResumption
        // 步骤 2），拒绝转 throw completion 走 throw 转发（步骤 3），成功后
        // 以 awaited 值走 return 转发（步骤 7.c）。
        let return_forward = {
            let received = self.emit_load_resume_val(return_unwrap);
            self.emit_delegate_await(return_unwrap, received, Some(throw_forward))?
        };
        {
            let received = self.emit_load_resume_val(return_forward);
            let iter = self.alloc_value();
            self.current_function.append_instruction(
                return_forward,
                Instruction::LoadVar {
                    dest: iter,
                    name: iter_ir_name.to_string(),
                },
            );
            let marker = self.alloc_value();
            self.current_function.append_instruction(
                return_forward,
                Instruction::CallBuiltin {
                    dest: Some(marker),
                    builtin: Builtin::AsyncIteratorDelegateReturn,
                    args: vec![iter, received],
                },
            );
            let after = self.lower_value_exception_branch(return_forward, marker)?;
            let kind = self.emit_get_string_prop(after, marker, "k");
            let payload = self.emit_get_string_prop(after, marker, "v");
            let is_missing = self.emit_eq_number(after, kind, 2.0);
            let await_received = self.current_function.new_block();
            let await_call = self.current_function.new_block();
            self.current_function.set_terminator(
                after,
                Terminator::Branch {
                    condition: is_missing,
                    true_block: await_received,
                    false_block: await_call,
                },
            );

            // k=2：方法缺失——Await(received)（步骤 7.c.iii.2）后
            // ReturnCompletion(awaited)。
            let resolved = self.emit_delegate_await(await_received, payload, None)?;
            let awaited = self.emit_load_resume_val(resolved);
            self.emit_delegate_return_completion(resolved, awaited)?;

            // k=0：Await(innerReturnResult)（步骤 7.c.v）→ 对象校验（步骤
            // 7.c.vi）→ done 分支：done 则 ReturnCompletion(? IteratorValue)
            // （步骤 7.c.viii），未完成回 body 继续 yield（步骤 7.c.ix）。
            let resolved = self.emit_delegate_await(await_call, payload, None)?;
            let inner_result = self.emit_load_resume_val(resolved);
            let checked = self.alloc_value();
            self.current_function.append_instruction(
                resolved,
                Instruction::CallBuiltin {
                    dest: Some(checked),
                    builtin: Builtin::IteratorResultRequireObject,
                    args: vec![inner_result],
                },
            );
            let resolved = self.lower_value_exception_branch(resolved, checked)?;
            let done = self.emit_get_string_prop(resolved, checked, "done");
            // 步骤 7.c.vii `? IteratorComplete`：done getter 抛出传播。
            let resolved = self.lower_value_exception_branch(resolved, done)?;
            let not_done = self.alloc_value();
            self.current_function.append_instruction(
                resolved,
                Instruction::Unary {
                    dest: not_done,
                    op: UnaryOp::Not,
                    value: done,
                },
            );
            let return_done = self.current_function.new_block();
            self.current_function.set_terminator(
                resolved,
                Terminator::Branch {
                    condition: not_done,
                    true_block: body,
                    false_block: return_done,
                },
            );
            let final_value = self.emit_get_string_prop(return_done, checked, "value");
            // 步骤 7.c.viii.1 `? IteratorValue`：value getter 抛出传播。
            let after_final = self.lower_value_exception_branch(return_done, final_value)?;
            self.emit_delegate_return_completion(after_final, final_value)?;
        }

        Ok(continue_block)
    }
}
