use super::*;

impl Lowerer {
    pub(crate) fn lower_await_expr(
        &mut self,
        await_expr: &swc_ast::AwaitExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let mut block = block;
        // AwaitExpression 的 `? GetValue(exprRef)`：操作数求值抛出必须先于
        // Await 传播（进入本地 try/catch 或 reject 返回的 promise），异常
        // 哨兵不得作为普通值流入 PromiseResolveStatic。
        let value = self.lower_call_operand_then_continue(&await_expr.arg, &mut block)?;

        let promised = self.alloc_value();
        {
            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(promised),
                    builtin: Builtin::PromiseResolveStatic,
                    args: vec![undef_val, value],
                },
            );
        }

        let next_state = self.async_state_counter;
        self.async_state_counter += 1;

        let resume_block = self.current_function.new_block();
        let reject_block = self.current_function.new_block();
        let continue_block = self.current_function.new_block();

        self.async_resume_blocks.push((next_state, resume_block));
        let visible_bindings = self.async_visible_binding_names();

        // 推迟 save/restore —— 由 resolve_pending_suspends 在函数体 lowering 完成后统一处理
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

        let one_const = self.module.add_constant(Constant::Number(1.0));
        let one_val = self.alloc_value();
        self.current_function.append_instruction(
            resume_block,
            Instruction::Const {
                dest: one_val,
                constant: one_const,
            },
        );
        let is_reject = self.alloc_value();
        self.current_function.append_instruction(
            resume_block,
            Instruction::Compare {
                dest: is_reject,
                op: CompareOp::StrictEq,
                lhs: is_rejected,
                rhs: one_val,
            },
        );
        self.current_function.set_terminator(
            resume_block,
            Terminator::Branch {
                condition: is_reject,
                true_block: reject_block,
                false_block: continue_block,
            },
        );

        self.emit_throw_value(reject_block, resume_val)?;
        let result = self.alloc_value();
        self.current_function.append_instruction(
            continue_block,
            Instruction::LoadVar {
                dest: result,
                name: format!("${}.$resume_val", self.async_resume_val_scope_id),
            },
        );
        self.await_continue_block = Some(continue_block);

        Ok(result)
    }

    pub(crate) fn lower_yield_expr(
        &mut self,
        yield_expr: &swc_ast::YieldExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let mut block = block;
        // YieldExpression 的 `? GetValue(exprRef)`：操作数求值抛出必须先于
        // yield 挂起传播（本地 try/catch 或 GeneratorThrow / AsyncGeneratorThrow）。
        let value = if let Some(arg) = &yield_expr.arg {
            self.lower_call_operand_then_continue(arg, &mut block)?
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

        if yield_expr.delegate && self.is_generator_fn {
            return self.lower_sync_yield_delegate(value, block, yield_expr.span());
        }

        self.lower_yield_value(value, block, yield_expr.span())
    }

    fn lower_sync_yield_delegate(
        &mut self,
        iterable: ValueId,
        block: BasicBlockId,
        span: swc_core::common::Span,
    ) -> Result<ValueId, LoweringError> {
        let iter_name = format!("$yield_star_iter_{}", self.next_temp);
        self.next_temp += 1;
        let scope_id = self
            .scopes
            .declare(&iter_name, VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let iter_ir_name = format!("${scope_id}.{iter_name}");

        let iter_handle = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(iter_handle),
                builtin: Builtin::IteratorFrom,
                args: vec![iterable],
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::StoreVar {
                name: iter_ir_name.clone(),
                value: iter_handle,
            },
        );

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
        let not_done = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::Unary {
                dest: not_done,
                op: UnaryOp::Not,
                value: done,
            },
        );
        self.current_function.set_terminator(
            header,
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
        let _resume_value = self.lower_yield_value(yielded_value, body, span)?;
        let after_yield = self.resolve_store_block(body);
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

        self.expr_merge_block = Some(exit);
        Ok(final_value)
    }

    fn lower_yield_value(
        &mut self,
        value: ValueId,
        block: BasicBlockId,
        span: swc_core::common::Span,
    ) -> Result<ValueId, LoweringError> {
        let gen_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: gen_val,
                name: format!("${}.$generator", self.async_generator_scope_id),
            },
        );

        if self.is_async_fn {
            let next_state = self.async_state_counter;
            self.async_state_counter += 1;

            let resume_block = self.current_function.new_block();
            let reject_block = self.current_function.new_block();
            let continue_block = self.current_function.new_block();
            let return_block = self.current_function.new_block();

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

            let resume_val = self.alloc_value();
            self.current_function.append_instruction(
                resume_block,
                Instruction::LoadVar {
                    dest: resume_val,
                    name: format!("${}.$resume_val", self.async_resume_val_scope_id),
                },
            );
            let completion = self.alloc_value();
            self.current_function.append_instruction(
                resume_block,
                Instruction::LoadVar {
                    dest: completion,
                    name: format!("${}.$is_rejected", self.async_is_rejected_scope_id),
                },
            );

            // 用嵌套 Branch 代替 Switch：completion == 1 → reject, == 2 → return, else → continue
            let one_const = self.module.add_constant(Constant::Number(1.0));
            let one_val = self.alloc_value();
            self.current_function.append_instruction(
                resume_block,
                Instruction::Const {
                    dest: one_val,
                    constant: one_const,
                },
            );
            let is_throw = self.alloc_value();
            self.current_function.append_instruction(
                resume_block,
                Instruction::Compare {
                    dest: is_throw,
                    op: CompareOp::StrictEq,
                    lhs: completion,
                    rhs: one_val,
                },
            );
            let check_return = self.current_function.new_block();
            self.current_function.set_terminator(
                resume_block,
                Terminator::Branch {
                    condition: is_throw,
                    true_block: reject_block,
                    false_block: check_return,
                },
            );

            // check_return: completion == 2 → return_block, else → continue_block
            let two_const = self.module.add_constant(Constant::Number(2.0));
            let two_val = self.alloc_value();
            self.current_function.append_instruction(
                check_return,
                Instruction::Const {
                    dest: two_val,
                    constant: two_const,
                },
            );
            let is_return = self.alloc_value();
            self.current_function.append_instruction(
                check_return,
                Instruction::Compare {
                    dest: is_return,
                    op: CompareOp::StrictEq,
                    lhs: completion,
                    rhs: two_val,
                },
            );
            self.current_function.set_terminator(
                check_return,
                Terminator::Branch {
                    condition: is_return,
                    true_block: return_block,
                    false_block: continue_block,
                },
            );

            self.emit_throw_value(reject_block, resume_val)?;
            let return_slot = self.preserve_suspending_completion(return_block, resume_val);

            match self.lower_pending_finalizers(return_block)? {
                StmtFlow::Open(after_finally) => {
                    let gen_val2 = self.alloc_value();
                    self.current_function.append_instruction(
                        after_finally,
                        Instruction::LoadVar {
                            dest: gen_val2,
                            name: format!("${}.$generator", self.async_generator_scope_id),
                        },
                    );
                    let return_value = self.reload_suspending_completion(
                        after_finally,
                        resume_val,
                        return_slot.as_deref(),
                    );
                    self.current_function.append_instruction(
                        after_finally,
                        Instruction::CallBuiltin {
                            dest: None,
                            builtin: Builtin::AsyncGeneratorReturn,
                            args: vec![gen_val2, return_value],
                        },
                    );
                    self.current_function
                        .set_terminator(after_finally, Terminator::Return { value: None });
                }
                StmtFlow::Terminated => {}
            }

            let result = self.alloc_value();
            self.current_function.append_instruction(
                continue_block,
                Instruction::LoadVar {
                    dest: result,
                    name: format!("${}.$resume_val", self.async_resume_val_scope_id),
                },
            );

            self.expr_merge_block = Some(continue_block);
            Ok(result)
        } else if self.is_generator_fn {
            let next_state = self.async_state_counter;
            self.async_state_counter += 1;

            let resume_block = self.current_function.new_block();
            let reject_block = self.current_function.new_block();
            let continue_block = self.current_function.new_block();
            let return_block = self.current_function.new_block();

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

            let resume_val = self.alloc_value();
            self.current_function.append_instruction(
                resume_block,
                Instruction::LoadVar {
                    dest: resume_val,
                    name: format!("${}.$resume_val", self.async_resume_val_scope_id),
                },
            );
            let completion = self.alloc_value();
            self.current_function.append_instruction(
                resume_block,
                Instruction::LoadVar {
                    dest: completion,
                    name: format!("${}.$is_rejected", self.async_is_rejected_scope_id),
                },
            );

            let one_const = self.module.add_constant(Constant::Number(1.0));
            let one_val = self.alloc_value();
            self.current_function.append_instruction(
                resume_block,
                Instruction::Const {
                    dest: one_val,
                    constant: one_const,
                },
            );
            let is_throw = self.alloc_value();
            self.current_function.append_instruction(
                resume_block,
                Instruction::Compare {
                    dest: is_throw,
                    op: CompareOp::StrictEq,
                    lhs: completion,
                    rhs: one_val,
                },
            );
            let check_return = self.current_function.new_block();
            self.current_function.set_terminator(
                resume_block,
                Terminator::Branch {
                    condition: is_throw,
                    true_block: reject_block,
                    false_block: check_return,
                },
            );

            let two_const = self.module.add_constant(Constant::Number(2.0));
            let two_val = self.alloc_value();
            self.current_function.append_instruction(
                check_return,
                Instruction::Const {
                    dest: two_val,
                    constant: two_const,
                },
            );
            let is_return = self.alloc_value();
            self.current_function.append_instruction(
                check_return,
                Instruction::Compare {
                    dest: is_return,
                    op: CompareOp::StrictEq,
                    lhs: completion,
                    rhs: two_val,
                },
            );
            self.current_function.set_terminator(
                check_return,
                Terminator::Branch {
                    condition: is_return,
                    true_block: return_block,
                    false_block: continue_block,
                },
            );

            self.emit_throw_value(reject_block, resume_val)?;
            let return_slot = self.preserve_suspending_completion(return_block, resume_val);

            match self.lower_pending_finalizers(return_block)? {
                StmtFlow::Open(after_finally) => {
                    let gen_val2 = self.alloc_value();
                    self.current_function.append_instruction(
                        after_finally,
                        Instruction::LoadVar {
                            dest: gen_val2,
                            name: format!("${}.$generator", self.async_generator_scope_id),
                        },
                    );
                    let return_value = self.reload_suspending_completion(
                        after_finally,
                        resume_val,
                        return_slot.as_deref(),
                    );
                    let final_result = self.alloc_value();
                    self.current_function.append_instruction(
                        after_finally,
                        Instruction::CallBuiltin {
                            dest: Some(final_result),
                            builtin: Builtin::GeneratorReturn,
                            args: vec![gen_val2, return_value],
                        },
                    );
                    self.current_function.set_terminator(
                        after_finally,
                        Terminator::Return {
                            value: Some(final_result),
                        },
                    );
                }
                StmtFlow::Terminated => {}
            }

            let yielded_result = self.alloc_value();
            self.current_function.append_instruction(
                continue_block,
                Instruction::LoadVar {
                    dest: yielded_result,
                    name: format!("${}.$resume_val", self.async_resume_val_scope_id),
                },
            );

            self.expr_merge_block = Some(continue_block);
            Ok(yielded_result)
        } else {
            Err(self.error(span, "yield outside generator"))
        }
    }

    pub(crate) fn lower_new_expr(
        &mut self,
        new_expr: &swc_ast::NewExpr,
        block: BasicBlockId,
    ) -> Result<(ValueId, BasicBlockId), LoweringError> {
        // 构造器名解析穿越 with 作用域时禁用全部编译期内建构造器快路径：
        // with 对象可能提供同名属性，callee 须经通用路径的 with 读分派解析。
        if let swc_ast::Expr::Ident(ident) = new_expr.callee.as_ref()
            && self.with_scopes_for_ident(ident.sym.as_ref()).is_empty()
        {
            // 词法/模块绑定（含导入别名与 TDZ 声明）遮蔽全局构造器名：
            // 被遮蔽时禁用全部编译期内建构造器快路径，走通用 ConstructCall。
            if ident.sym == "Promise" && !self.global_intrinsic_shadowed(&ident.sym) {
                return self.lower_new_promise(new_expr, block);
            }
            if ident.sym == "Proxy" && !self.global_intrinsic_shadowed(&ident.sym) {
                // new Proxy(target, handler) → CallBuiltin(ProxyCreate, [target, handler])
                let mut call_block = block;
                let arg_vals =
                    self.lower_construct_args(new_expr.args.as_deref(), &mut call_block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    call_block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::ProxyCreate,
                        args: arg_vals,
                    },
                );
                return Ok((dest, call_block));
            }
            if ident.sym == "RegExp" && !self.global_intrinsic_shadowed(&ident.sym) {
                let mut call_block = block;
                let callee_val =
                    self.lower_expr_then_continue(&new_expr.callee, &mut call_block)?;

                let this_val = self.alloc_value();
                self.current_function.append_instruction(
                    call_block,
                    Instruction::NewObject {
                        dest: this_val,
                        capacity: 0,
                    },
                );

                let arg_vals =
                    self.lower_construct_args(new_expr.args.as_deref(), &mut call_block)?;

                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    call_block,
                    Instruction::ConstructCall {
                        dest: Some(dest),
                        callee: callee_val,
                        this_val,
                        args: arg_vals,
                    },
                );

                let continue_block = self.lower_value_exception_branch(call_block, dest)?;
                return Ok((dest, continue_block));
            }
            // WeakRef / FinalizationRegistry constructors (can throw — need exception checking)
            if !self.global_intrinsic_shadowed(&ident.sym)
                && let Some(builtin) = builtin_from_global_ident(&ident.sym)
                && matches!(
                    builtin,
                    Builtin::WeakRefConstructor
                        | Builtin::FinalizationRegistryConstructor
                        | Builtin::HeadersConstructor
                        | Builtin::RequestConstructor
                        | Builtin::ResponseConstructor
                        | Builtin::AbortControllerConstructor
                        | Builtin::ReadableStreamConstructor
                        | Builtin::WritableStreamConstructor
                        | Builtin::TransformStreamConstructor
                        | Builtin::CountQueuingStrategyConstructor
                        | Builtin::ByteLengthQueuingStrategyConstructor
                )
            {
                let mut call_block = block;
                let mut arg_vals =
                    self.lower_construct_args(new_expr.args.as_deref(), &mut call_block)?;
                if arg_vals.is_empty() {
                    arg_vals.push({
                        let c = self.module.add_constant(Constant::Undefined);
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            call_block,
                            Instruction::Const { dest, constant: c },
                        );
                        dest
                    });
                }
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    call_block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin,
                        args: arg_vals,
                    },
                );
                // Exception check
                let is_exc = self.alloc_value();
                self.current_function.append_instruction(
                    call_block,
                    Instruction::IsException {
                        dest: is_exc,
                        value: dest,
                    },
                );
                let continue_block = self.current_function.new_block();
                let exc_block = self.current_function.new_block();
                self.current_function.set_terminator(
                    call_block,
                    Terminator::Branch {
                        condition: is_exc,
                        true_block: exc_block,
                        false_block: continue_block,
                    },
                );
                // Exception path: unwrap and throw
                let thrown_val = self.alloc_value();
                self.current_function.append_instruction(
                    exc_block,
                    Instruction::CallBuiltin {
                        dest: Some(thrown_val),
                        builtin: Builtin::ExceptionValue,
                        args: vec![dest],
                    },
                );
                self.emit_throw_value(exc_block, thrown_val)?;
                return Ok((dest, continue_block));
            }
            // 这些宿主构造器当前直接返回宿主对象；Error 构造器不能走这里，
            // 它们需要通用 ConstructCall 传入 new.target 并把已分配 receiver 初始化为错误对象。
            if !self.global_intrinsic_shadowed(&ident.sym)
                && let Some(builtin) = builtin_from_global_ident(&ident.sym)
                && matches!(
                    builtin,
                    Builtin::MapConstructor
                        | Builtin::SetConstructor
                        | Builtin::WeakMapConstructor
                        | Builtin::WeakSetConstructor
                        | Builtin::DateConstructor
                        | Builtin::ArrayBufferConstructor
                        | Builtin::SharedArrayBufferConstructor
                        | Builtin::DataViewConstructor
                        | Builtin::Int8ArrayConstructor
                        | Builtin::Uint8ArrayConstructor
                        | Builtin::Uint8ClampedArrayConstructor
                        | Builtin::Int16ArrayConstructor
                        | Builtin::Uint16ArrayConstructor
                        | Builtin::Int32ArrayConstructor
                        | Builtin::Uint32ArrayConstructor
                        | Builtin::Float32ArrayConstructor
                        | Builtin::Float64ArrayConstructor
                        | Builtin::BigInt64ArrayConstructor
                        | Builtin::BigUint64ArrayConstructor
                )
            {
                let mut call_block = block;
                let mut arg_vals =
                    self.lower_construct_args(new_expr.args.as_deref(), &mut call_block)?;
                if arg_vals.is_empty() {
                    arg_vals.push({
                        let c = self.module.add_constant(Constant::Undefined);
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            call_block,
                            Instruction::Const { dest, constant: c },
                        );
                        dest
                    });
                }
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    call_block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: if builtin == Builtin::DateConstructor {
                            Builtin::DateConstructorNew
                        } else {
                            builtin
                        },
                        args: arg_vals,
                    },
                );
                return Ok((dest, call_block));
            }
        }

        let mut call_block = block;
        // 构造器求值（`new o.C()` 的 getter 等）抛出必须先于 Construct 分叉
        // 传播，哨兵不得作为 callee 流入（否则误报 "... is not a constructor"）。
        let callee_val =
            self.lower_call_operand_then_continue(&new_expr.callee, &mut call_block)?;
        if let Some(args) = new_expr.args.as_deref()
            && Self::call_args_have_spread(args)
        {
            let (args_array, end_block) = self.lower_call_args_to_array(args, call_block)?;
            call_block = end_block;
            let result = self.alloc_value();
            self.current_function.append_instruction(
                call_block,
                Instruction::CallBuiltin {
                    dest: Some(result),
                    builtin: Builtin::ReflectConstruct,
                    args: vec![callee_val, args_array, callee_val],
                },
            );
            call_block = self.lower_value_exception_branch(call_block, result)?;
            return Ok((result, call_block));
        }

        // Create new object. Error 构造器需要更大容量以容纳 name/message/__error_brand__/cause/stack。
        let new_obj_capacity = match new_expr.callee.as_ref() {
            swc_ast::Expr::Ident(ident) if !self.global_intrinsic_shadowed(&ident.sym) => {
                match builtin_from_global_ident(&ident.sym) {
                    Some(
                        Builtin::ErrorConstructor
                        | Builtin::TypeErrorConstructor
                        | Builtin::RangeErrorConstructor
                        | Builtin::SyntaxErrorConstructor
                        | Builtin::ReferenceErrorConstructor
                        | Builtin::URIErrorConstructor
                        | Builtin::EvalErrorConstructor,
                    ) => 6,
                    _ => 4,
                }
            }
            _ => 4,
        };
        let obj_val = self.alloc_value();
        self.current_function.append_instruction(
            call_block,
            Instruction::NewObject {
                dest: obj_val,
                capacity: new_obj_capacity,
            },
        );

        // Get prototype from constructor via GetPrototypeFromConstructor builtin.
        // 语义等价于 ECMAScript GetPrototypeFromConstructor(F)：
        // 1. 读取 ctor.prototype（含原型链遍历）
        // 2. 若非 Object 类型（包含 Array、Function、Closure 等），回退到 Object.prototype
        let proto_val = self.alloc_value();
        self.current_function.append_instruction(
            call_block,
            Instruction::CallBuiltin {
                dest: Some(proto_val),
                builtin: Builtin::GetPrototypeFromConstructor,
                args: vec![callee_val],
            },
        );

        // Set __proto__ on the new object directly via SetProto.
        self.current_function.append_instruction(
            call_block,
            Instruction::SetProto {
                object: obj_val,
                value: proto_val,
            },
        );

        // Lower arguments（实参抛出即中止构造并传播）。
        let arg_vals = self.lower_construct_args(new_expr.args.as_deref(), &mut call_block)?;

        // Call the constructor with the new object as `this`.
        let ctor_result = self.alloc_value();
        self.current_function.append_instruction(
            call_block,
            Instruction::ConstructCall {
                dest: Some(ctor_result),
                callee: callee_val,
                this_val: obj_val,
                args: arg_vals,
            },
        );

        let (result, end_block) = self.select_construct_result(call_block, ctor_result, obj_val);
        // Construct 异常（构造器抛出 / callee 非构造器）分叉传播。
        let continue_block = self.lower_value_exception_branch(end_block, result)?;
        Ok((result, continue_block))
    }
}
