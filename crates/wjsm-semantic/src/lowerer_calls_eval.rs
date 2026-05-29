use super::*;

impl Lowerer {
    pub(crate) fn lower_call_expr(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let callee_val: ValueId;
        let this_val: ValueId;

        match &call.callee {
            swc_ast::Callee::Expr(expr) => {
                if let swc_ast::Expr::Ident(ident) = expr.as_ref() {
                    if ident.sym.as_ref() == "eval" && self.scopes.lookup("eval").is_err() {
                        let (val, merge_block) = self.lower_direct_eval_call(call, block)?;
                        self.eval_continue_block = Some(merge_block);
                        return Ok(val);
                    }
                    if let Some(builtin) = builtin_from_global_ident(&ident.sym)
                        && self.scopes.lookup(&ident.sym).is_err()
                    {
                        return self.lower_host_builtin_call_expr(call, block, builtin);
                    }
                }

                // 检测 MemberExpr 被调用者 → 提取 obj 作为 this
                if let swc_ast::Expr::Member(member_expr) = expr.as_ref() {
                    // 静态宿主 API（console.*, Object.*, JSON.*）不读取对象本身。
                    if let swc_ast::Expr::Ident(obj_ident) = member_expr.obj.as_ref()
                        && let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
                        && let Some(builtin) =
                            builtin_from_static_member(&obj_ident.sym, &prop_ident.sym)
                        && self.scopes.lookup(&obj_ident.sym).is_err()
                    {
                        // Promise 静态方法需要传递构造器作为第一个参数（species-aware）
                        if matches!(
                            builtin,
                            Builtin::PromiseResolveStatic
                                | Builtin::PromiseRejectStatic
                                | Builtin::PromiseAll
                                | Builtin::PromiseRace
                                | Builtin::PromiseAllSettled
                                | Builtin::PromiseAny
                                | Builtin::PromiseWithResolvers
                        ) {
                            let undef_const = self.module.add_constant(Constant::Undefined);
                            let constructor_val = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::Const {
                                    dest: constructor_val,
                                    constant: undef_const,
                                },
                            );
                            let mut args = vec![constructor_val];
                            for arg in &call.args {
                                args.push(self.lower_expr(&arg.expr, block)?);
                            }
                            // 无参数时补 undefined
                            if args.len() == 1 {
                                let undef_val = self.alloc_value();
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::Const {
                                        dest: undef_val,
                                        constant: undef_const,
                                    },
                                );
                                args.push(undef_val);
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin,
                                    args,
                                },
                            );
                            return Ok(dest);
                        }
                        return self.lower_host_builtin_call_expr(call, block, builtin);
                    }

                    // TypedArray.prototype 方法调用优化（必须在 String 之前，因为 at/indexOf/includes/toString
                    // 在 String 和 TypedArray 上同名）。仅在 receiver 是已知 TypedArray 绑定时启用，
                    // 避免错误拦截普通字符串的同名方法调用。
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
                        && let Some(ta_builtin) =
                            builtin_from_typedarray_proto_method(&prop_ident.sym)
                        && let swc_ast::Expr::Ident(receiver_ident) = member_expr.obj.as_ref()
                        && self.is_typedarray_binding(receiver_ident)
                    {
                        this_val = self.lower_expr(&member_expr.obj, block)?;
                        let mut builtin_args = vec![this_val];
                        for arg in &call.args {
                            builtin_args.push(self.lower_expr(&arg.expr, block)?);
                        }
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: ta_builtin,
                                args: builtin_args,
                            },
                        );
                        return Ok(dest);
                    }
                    // String.prototype 方法调用优化（必须在 Array 之前，因为 at/slice/concat 等方法在 String 和 Array 上同名）
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
                        && let Some(string_builtin) =
                            builtin_from_string_proto_method(&prop_ident.sym)
                    {
                        let _ = builtin_call_signature(string_builtin);
                        this_val = self.lower_expr(&member_expr.obj, block)?;
                        let mut builtin_args = vec![this_val];
                        for arg in &call.args {
                            builtin_args.push(self.lower_expr(&arg.expr, block)?);
                        }
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: string_builtin,
                                args: builtin_args,
                            },
                        );
                        return Ok(dest);
                    }

                    // RegExp.prototype 方法调用优化：RegExp 宿主函数使用固定二参调用约定，
                    // 不能通过运行时属性查找后再走通用 call_indirect。
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
                        && let Some(regexp_builtin) =
                            builtin_from_regexp_proto_method(&prop_ident.sym)
                    {
                        this_val = self.lower_expr(&member_expr.obj, block)?;
                        let mut builtin_args = vec![this_val];
                        if let Some(arg) = call.args.first() {
                            builtin_args.push(self.lower_expr(&arg.expr, block)?);
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
                            builtin_args.push(undef_val);
                        }
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: regexp_builtin,
                                args: builtin_args,
                            },
                        );
                        return Ok(dest);
                    }

                    // Array.prototype 方法调用优化：发出 CallBuiltin 代替 Call，
                    // 跳过运行时属性解析（原型链查找）。
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop {
                        if let Some(array_builtin) =
                            builtin_from_array_proto_method(&prop_ident.sym)
                        {
                            // obj.method() → obj 是 this
                            this_val = self.lower_expr(&member_expr.obj, block)?;
                            let mut builtin_args = vec![this_val];
                            for arg in &call.args {
                                builtin_args.push(self.lower_expr(&arg.expr, block)?);
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: array_builtin,
                                    args: builtin_args,
                                },
                            );
                            return Ok(dest);
                        }

                        // TypedArray.prototype 方法调用优化：发出 CallBuiltin 代替 Call，
                        // 跳过运行时属性解析。
                        if let Some(ta_builtin) =
                            builtin_from_typedarray_proto_method(&prop_ident.sym)
                            && let swc_ast::Expr::Ident(receiver_ident) = member_expr.obj.as_ref()
                            && self.is_typedarray_binding(receiver_ident)
                        {
                            this_val = self.lower_expr(&member_expr.obj, block)?;
                            let mut builtin_args = vec![this_val];
                            for arg in &call.args {
                                builtin_args.push(self.lower_expr(&arg.expr, block)?);
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: ta_builtin,
                                    args: builtin_args,
                                },
                            );
                            return Ok(dest);
                        }

                        // Function.prototype.call/apply/bind: func.call(thisArg, ...args)
                        if let Some(func_builtin) =
                            builtin_from_function_proto_method(&prop_ident.sym)
                        {
                            // 特殊优化: Object.prototype.toString.call(obj) → CallBuiltin(ObjectProtoToString, obj)
                            // 跳过运行时原型链查找
                            if matches!(func_builtin, Builtin::FuncCall) {
                                if let Some(object_proto_builtin) =
                                    self.is_object_proto_method_access(&member_expr.obj)
                                {
                                    // Object.prototype.toString.call(thisArg) → ObjectProtoToString(thisArg)
                                    let this_arg = if let Some(first_arg) = call.args.first() {
                                        self.lower_expr(&first_arg.expr, block)?
                                    } else {
                                        let undef_const =
                                            self.module.add_constant(Constant::Undefined);
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
                                    let dest = self.alloc_value();
                                    self.current_function.append_instruction(
                                        block,
                                        Instruction::CallBuiltin {
                                            dest: Some(dest),
                                            builtin: object_proto_builtin,
                                            args: vec![this_arg],
                                        },
                                    );
                                    return Ok(dest);
                                }
                            }

                            let func_val = self.lower_expr(&member_expr.obj, block)?;
                            let mut builtin_args = vec![func_val];

                            if matches!(func_builtin, Builtin::FuncApply) {
                                // func.apply(thisArg, argsArray)
                                if let Some(first_arg) = call.args.first() {
                                    builtin_args.push(self.lower_expr(&first_arg.expr, block)?);
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
                                    builtin_args.push(undef_val);
                                }
                                if call.args.len() > 1 {
                                    builtin_args.push(self.lower_expr(&call.args[1].expr, block)?);
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
                                    builtin_args.push(undef_val);
                                }
                            } else {
                                // func.call(thisArg, ...restArgs) / func.bind(thisArg, ...boundArgs)
                                for arg in &call.args {
                                    builtin_args.push(self.lower_expr(&arg.expr, block)?);
                                }
                                // Ensure at least thisArg (first arg after func) exists
                                if call.args.is_empty() {
                                    let undef_const = self.module.add_constant(Constant::Undefined);
                                    let undef_val = self.alloc_value();
                                    self.current_function.append_instruction(
                                        block,
                                        Instruction::Const {
                                            dest: undef_val,
                                            constant: undef_const,
                                        },
                                    );
                                    builtin_args.push(undef_val);
                                }
                            }

                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: func_builtin,
                                    args: builtin_args,
                                },
                            );
                            return Ok(dest);
                        }

                        // Object.prototype 方法调用优化：hasOwnProperty
                        if let Some(obj_proto_builtin) =
                            builtin_from_object_proto_method(&prop_ident.sym)
                        {
                            // obj.method() → obj 是 this
                            let this_val = self.lower_expr(&member_expr.obj, block)?;
                            let mut builtin_args = vec![this_val];
                            for arg in &call.args {
                                builtin_args.push(self.lower_expr(&arg.expr, block)?);
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: obj_proto_builtin,
                                    args: builtin_args,
                                },
                            );
                            return Ok(dest);
                        }

                        if let Some(promise_proto_builtin) =
                            builtin_from_promise_proto_method(&prop_ident.sym)
                        {
                            this_val = self.lower_expr(&member_expr.obj, block)?;
                            let mut builtin_args = vec![this_val];
                            for arg in &call.args {
                                builtin_args.push(self.lower_expr(&arg.expr, block)?);
                            }
                            if builtin_args.len() < 3
                                && matches!(promise_proto_builtin, Builtin::PromiseThen)
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
                                builtin_args.push(undef_val);
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: promise_proto_builtin,
                                    args: builtin_args,
                                },
                            );
                            return Ok(dest);
                        }

                        if let Some(number_proto_builtin) =
                            builtin_from_number_proto_method(&prop_ident.sym)
                        {
                            this_val = self.lower_expr(&member_expr.obj, block)?;
                            let mut builtin_args = vec![this_val];
                            for arg in &call.args {
                                builtin_args.push(self.lower_expr(&arg.expr, block)?);
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: number_proto_builtin,
                                    args: builtin_args,
                                },
                            );
                            return Ok(dest);
                        }

                        if let Some(boolean_proto_builtin) =
                            builtin_from_boolean_proto_method(&prop_ident.sym)
                        {
                            this_val = self.lower_expr(&member_expr.obj, block)?;
                            let mut builtin_args = vec![this_val];
                            for arg in &call.args {
                                builtin_args.push(self.lower_expr(&arg.expr, block)?);
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: boolean_proto_builtin,
                                    args: builtin_args,
                                },
                            );
                            return Ok(dest);
                        }

                        if let Some(error_proto_builtin) =
                            builtin_from_error_proto_method(&prop_ident.sym)
                        {
                            this_val = self.lower_expr(&member_expr.obj, block)?;
                            let mut builtin_args = vec![this_val];
                            for arg in &call.args {
                                builtin_args.push(self.lower_expr(&arg.expr, block)?);
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: error_proto_builtin,
                                    args: builtin_args,
                                },
                            );
                            return Ok(dest);
                        }
                    }

                    // obj.method() → obj 是 this，method 是 callee（未被拦截时）
                    this_val = self.lower_expr(&member_expr.obj, block)?;
                    callee_val = self.lower_member_expr(member_expr, block)?;
                } else {
                    // 普通调用 → this = undefined
                    let undef_const = self.module.add_constant(Constant::Undefined);
                    this_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: this_val,
                            constant: undef_const,
                        },
                    );
                    callee_val = self.lower_expr(expr, block)?;
                }
            }
            swc_ast::Callee::Import { .. } => {
                // 动态 import() 调用
                return self.lower_dynamic_import_call(call, block);
            }
            swc_ast::Callee::Super(_) => {
                return Err(self.error(call.span, "super call is not supported"));
            }
        }

        // 性能优化：预分配容量避免循环中多次 reallocation
        let mut call_block = self.resolve_store_block(block);
        let mut args = Vec::with_capacity(call.args.len());
        for arg in &call.args {
            let arg_val = self.lower_expr_then_continue(&arg.expr, &mut call_block)?;
            args.push(arg_val);
        }

        let dest = self.alloc_value();
        self.current_function.append_instruction(
            call_block,
            Instruction::Call {
                dest: Some(dest),
                callee: callee_val,
                this_val,
                args,
            },
        );
        Ok(dest)
    }

    pub(crate) fn lower_direct_eval_call(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<(ValueId, BasicBlockId), LoweringError> {
        let mut eval_block = block;
        self.current_function.mark_has_eval();

        // 1. Lower the code argument
        let code_val = if let Some(first_arg) = call.args.first() {
            self.lower_expr_then_continue(&first_arg.expr, &mut eval_block)?
        } else {
            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                eval_block,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );
            undef_val
        };

        // 2. Get all lexically visible bindings (including TDZ)
        let all_bindings: Vec<_> = self
            .scopes
            .visible_bindings_all()
            .into_iter()
            .filter(|(_, name, _, _)| !matches!(name.as_str(), "undefined" | "NaN" | "Infinity"))
            .collect();

        // 3. Create ScopeRecord
        let capacity = self.const_val_i64(eval_block, all_bindings.len() as i64);
        let scope_record = self.alloc_value();
        self.current_function.append_instruction(
            eval_block,
            Instruction::CallBuiltin {
                dest: Some(scope_record),
                builtin: Builtin::ScopeRecordCreate,
                args: vec![capacity],
            },
        );

        // Store scope_record into $eval_env so the eval module can find it
        self.current_function.append_instruction(
            eval_block,
            Instruction::StoreVar {
                name: EVAL_SCOPE_ENV_PARAM.to_string(),
                value: scope_record,
            },
        );

        // 4. Add each binding to the ScopeRecord
        for (scope_id, name, kind, is_initialised) in &all_bindings {
            let name_const = self.module.add_constant(Constant::String(name.clone()));
            let name_val = self.alloc_value();
            self.current_function.append_instruction(
                eval_block,
                Instruction::Const {
                    dest: name_val,
                    constant: name_const,
                },
            );

            let binding = CapturedBinding::new(name.clone(), *scope_id);
            let value = if !self.binding_belongs_to_current_function(&binding)
                || self.is_shared_binding(&binding)
            {
                self.load_captured_binding(eval_block, &binding)?
            } else {
                let value = self.alloc_value();
                self.current_function.append_instruction(
                    eval_block,
                    Instruction::LoadVar {
                        dest: value,
                        name: binding.var_ir_name(),
                    },
                );
                value
            };

            let is_tdz = self.const_val_i64(eval_block, if *is_initialised { 0 } else { 1 });
            let is_const = self.const_val_i64(
                eval_block,
                if matches!(kind, VarKind::Const) { 1 } else { 0 },
            );

            self.current_function.append_instruction(
                eval_block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ScopeRecordAddBinding,
                    args: vec![scope_record, name_val, value, is_tdz, is_const],
                },
            );
        }

        // 5. Set meta: strict mode (key=0)
        let strict_key = self.const_val_i64(eval_block, 0);
        let strict_val = self.const_val_i64(eval_block, if self.strict_mode { 1 } else { 0 });
        self.current_function.append_instruction(
            eval_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ScopeRecordSetMeta,
                args: vec![scope_record, strict_key, strict_val],
            },
        );

        // 6. Set meta: has_arguments (key=1)
        let args_key = self.const_val_i64(eval_block, 1);
        let args_val = self.const_val_i64(
            eval_block,
            if self.eval_caller_has_arguments { 1 } else { 0 },
        );
        self.current_function.append_instruction(
            eval_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ScopeRecordSetMeta,
                args: vec![scope_record, args_key, args_val],
            },
        );

        // 7. Call Eval(code, scope_record)
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            eval_block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::Eval,
                args: vec![code_val, scope_record],
            },
        );

        // 8. Exception check
        let is_exc = self.alloc_value();
        self.current_function.append_instruction(
            eval_block,
            Instruction::IsException {
                dest: is_exc,
                value: dest,
            },
        );
        let continue_block = self.current_function.new_block();
        let exc_block = self.current_function.new_block();
        self.current_function.set_terminator(
            eval_block,
            Terminator::Branch {
                condition: is_exc,
                true_block: exc_block,
                false_block: continue_block,
            },
        );

        // 9. Exception path
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

        // 10. Writeback: for each initialised binding, read from ScopeRecord
        for (scope_id, name, _, _) in &all_bindings {
            let binding = CapturedBinding::new(name.clone(), *scope_id);

            let name_const = self.module.add_constant(Constant::String(name.clone()));
            let name_val = self.alloc_value();
            self.current_function.append_instruction(
                continue_block,
                Instruction::Const {
                    dest: name_val,
                    constant: name_const,
                },
            );

            let value = self.alloc_value();
            self.current_function.append_instruction(
                continue_block,
                Instruction::CallBuiltin {
                    dest: Some(value),
                    builtin: Builtin::EvalGetBinding,
                    args: vec![scope_record, name_val],
                },
            );

            if self.binding_belongs_to_current_function(&binding) {
                if self.is_shared_binding(&binding) {
                    // Shared env: write back via SetProp on the shared env
                    let env_val = self
                        .shared_env_value()
                        .expect("shared binding must have materialized env");
                    let key_val = self.append_env_key_const(continue_block, &binding);
                    self.current_function.append_instruction(
                        continue_block,
                        Instruction::SetProp {
                            object: env_val,
                            key: key_val,
                            value,
                        },
                    );
                } else {
                    // Direct local var
                    self.current_function.append_instruction(
                        continue_block,
                        Instruction::StoreVar {
                            name: binding.var_ir_name(),
                            value,
                        },
                    );
                }
            } else {
                // Captured binding from enclosing function: SetProp on env
                self.record_capture(binding.clone());
                let env_val = self.load_env_object(continue_block);
                let key_val = self.append_env_key_const(continue_block, &binding);
                self.current_function.append_instruction(
                    continue_block,
                    Instruction::SetProp {
                        object: env_val,
                        key: key_val,
                        value,
                    },
                );
            }
        }

        // 11. Destroy ScopeRecord
        self.current_function.append_instruction(
            continue_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ScopeRecordDestroy,
                args: vec![scope_record],
            },
        );

        let merge_block = self.current_function.new_block();
        self.current_function.set_terminator(
            continue_block,
            Terminator::Jump {
                target: merge_block,
            },
        );

        Ok((dest, merge_block))
    }

    pub(crate) fn eval_scope_bridge_active(&self) -> bool {
        self.eval_mode && self.eval_has_scope_bridge
    }

    pub(crate) fn load_eval_scope_env(&mut self, block: BasicBlockId) -> ValueId {
        let env = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: env,
                name: EVAL_SCOPE_ENV_PARAM.to_string(),
            },
        );
        env
    }

    pub(crate) fn append_eval_env_key_const(&mut self, block: BasicBlockId, name: &str) -> ValueId {
        let key_const = self.module.add_constant(Constant::String(name.to_string()));
        let key = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: key,
                constant: key_const,
            },
        );
        key
    }

    pub(crate) fn lower_eval_env_read(&mut self, name: &str, block: BasicBlockId) -> ValueId {
        let env = self.load_eval_scope_env(block);
        if self.eval_scope_record {
            let name_const = self.module.add_constant(Constant::String(name.to_string()));
            let name_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: name_val,
                    constant: name_const,
                },
            );
            let dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(dest),
                    builtin: Builtin::EvalGetBinding,
                    args: vec![env, name_val],
                },
            );
            dest
        } else {
            let key = self.append_eval_env_key_const(block, name);
            let dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::GetProp {
                    dest,
                    object: env,
                    key,
                },
            );
            dest
        }
    }

    pub(crate) fn append_eval_env_write(
        &mut self,
        name: &str,
        value: ValueId,
        block: BasicBlockId,
    ) {
        if !self.eval_scope_bridge_active() {
            return;
        }
        let env = self.load_eval_scope_env(block);
        if self.eval_scope_record {
            let name_const = self.module.add_constant(Constant::String(name.to_string()));
            let name_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: name_val,
                    constant: name_const,
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::EvalSetBinding,
                    args: vec![env, name_val, value],
                },
            );
        } else {
            let key = self.append_eval_env_key_const(block, name);
            self.current_function.append_instruction(
                block,
                Instruction::SetProp {
                    object: env,
                    key,
                    value,
                },
            );
        }
    }
    fn const_val_i64(&mut self, block: BasicBlockId, value: i64) -> ValueId {
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest,
                constant: self.module.add_constant(Constant::Number(value as f64)),
            },
        );
        dest
    }

    fn const_val(&mut self, block: BasicBlockId, constant: ConstantId) -> ValueId {
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        dest
    }

    /// 检测表达式是否为 Object.prototype.toString 或 Object.prototype.valueOf
    /// 用于优化 Function.prototype.call 调用模式
    fn is_object_proto_method_access(&self, expr: &swc_ast::Expr) -> Option<Builtin> {
        // 检测模式: Object.prototype.toString 或 Object.prototype.valueOf
        if let swc_ast::Expr::Member(outer_member) = expr {
            if let swc_ast::Expr::Member(inner_member) = outer_member.obj.as_ref() {
                if let swc_ast::Expr::Ident(obj_ident) = inner_member.obj.as_ref() {
                    if obj_ident.sym.as_ref() == "Object" {
                        if let swc_ast::MemberProp::Ident(proto_prop) = &inner_member.prop {
                            if proto_prop.sym.as_ref() == "prototype" {
                                if let swc_ast::MemberProp::Ident(method_prop) = &outer_member.prop
                                {
                                    return match method_prop.sym.as_str() {
                                        "toString" => Some(Builtin::ObjectProtoToString),
                                        "valueOf" => Some(Builtin::ObjectProtoValueOf),
                                        _ => None,
                                    };
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }
}
