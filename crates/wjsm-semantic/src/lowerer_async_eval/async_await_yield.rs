use super::*;

impl Lowerer {
    pub(crate) fn lower_await_expr(
        &mut self,
        await_expr: &swc_ast::AwaitExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let value = self.lower_expr(&await_expr.arg, block)?;

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

        self.current_function.set_terminator(
            resume_block,
            Terminator::Branch {
                condition: is_rejected,
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
        let value = if let Some(arg) = &yield_expr.arg {
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
            let is_rejected = self.alloc_value();
            self.current_function.append_instruction(
                resume_block,
                Instruction::LoadVar {
                    dest: is_rejected,
                    name: format!("${}.$is_rejected", self.async_is_rejected_scope_id),
                },
            );

            self.current_function.set_terminator(
                resume_block,
                Terminator::Branch {
                    condition: is_rejected,
                    true_block: reject_block,
                    false_block: continue_block,
                },
            );

            let gen_for_throw = self.alloc_value();
            self.current_function.append_instruction(
                reject_block,
                Instruction::LoadVar {
                    dest: gen_for_throw,
                    name: format!("${}.$generator", self.async_generator_scope_id),
                },
            );
            self.current_function.append_instruction(
                reject_block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::AsyncGeneratorThrow,
                    args: vec![gen_for_throw, resume_val],
                },
            );
            self.current_function
                .set_terminator(reject_block, Terminator::Return { value: None });

            let result = self.alloc_value();
            self.current_function.append_instruction(
                continue_block,
                Instruction::LoadVar {
                    dest: result,
                    name: format!("${}.$resume_val", self.async_resume_val_scope_id),
                },
            );

            Ok(result)
        } else {
            let result = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(result),
                    builtin: Builtin::AsyncGeneratorNext,
                    args: vec![gen_val, value],
                },
            );
            Ok(result)
        }
    }

    pub(crate) fn lower_new_expr(
        &mut self,
        new_expr: &swc_ast::NewExpr,
        block: BasicBlockId,
    ) -> Result<(ValueId, BasicBlockId), LoweringError> {
        if let swc_ast::Expr::Ident(ident) = new_expr.callee.as_ref() {
            if ident.sym == "Promise" && self.scopes.lookup(&ident.sym).is_err() {
                return Ok((self.lower_new_promise(new_expr, block)?, block));
            }
            if ident.sym == "Proxy" && self.scopes.lookup(&ident.sym).is_err() {
                // new Proxy(target, handler) → CallBuiltin(ProxyCreate, [target, handler])
                let mut arg_vals = Vec::new();
                if let Some(args) = &new_expr.args {
                    for arg in args {
                        let arg_val = self.lower_expr(&arg.expr, block)?;
                        arg_vals.push(arg_val);
                    }
                }
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::ProxyCreate,
                        args: arg_vals,
                    },
                );
                return Ok((dest, block));
            }
            // WeakRef / FinalizationRegistry constructors (can throw — need exception checking)
            if self.scopes.lookup(&ident.sym).is_err()
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
                let mut arg_vals = Vec::new();
                if let Some(args) = &new_expr.args {
                    for arg in args {
                        let arg_val = self.lower_expr(&arg.expr, block)?;
                        arg_vals.push(arg_val);
                    }
                }
                if arg_vals.is_empty() {
                    arg_vals.push({
                        let c = self.module.add_constant(Constant::Undefined);
                        let dest = self.alloc_value();
                        self.current_function
                            .append_instruction(block, Instruction::Const { dest, constant: c });
                        dest
                    });
                }
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin,
                        args: arg_vals,
                    },
                );
                // Exception check
                let is_exc = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::IsException {
                        dest: is_exc,
                        value: dest,
                    },
                );
                let continue_block = self.current_function.new_block();
                let exc_block = self.current_function.new_block();
                self.current_function.set_terminator(
                    block,
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
            if self.scopes.lookup(&ident.sym).is_err()
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
                let mut arg_vals = Vec::new();
                if let Some(args) = &new_expr.args {
                    for arg in args {
                        let arg_val = self.lower_expr(&arg.expr, block)?;
                        arg_vals.push(arg_val);
                    }
                }
                if arg_vals.is_empty() {
                    arg_vals.push({
                        let c = self.module.add_constant(Constant::Undefined);
                        let dest = self.alloc_value();
                        self.current_function
                            .append_instruction(block, Instruction::Const { dest, constant: c });
                        dest
                    });
                }
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
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
                return Ok((dest, block));
            }
        }

        let mut call_block = block;
        let callee_val = self.lower_expr_then_continue(&new_expr.callee, &mut call_block)?;

        // Create new object.
        let obj_val = self.alloc_value();
        self.current_function.append_instruction(
            call_block,
            Instruction::NewObject {
                dest: obj_val,
                capacity: 4,
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

        // Lower arguments.
        // 性能优化：预分配容量避免循环中多次 reallocation
        let cap = new_expr.args.as_ref().map_or(0, |a| a.len());
        let mut arg_vals = Vec::with_capacity(cap);
        if let Some(args) = &new_expr.args {
            for arg in args {
                let arg_val = self.lower_expr_then_continue(&arg.expr, &mut call_block)?;
                arg_vals.push(arg_val);
            }
        }

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

        let (result, end_block) =
            self.select_construct_result(call_block, ctor_result, obj_val);
        Ok((result, end_block))
    }
}
