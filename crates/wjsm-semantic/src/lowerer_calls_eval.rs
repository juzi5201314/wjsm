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

                        // Function.prototype.call/apply/bind: func.call(thisArg, ...args)
                        if let Some(func_builtin) =
                            builtin_from_function_proto_method(&prop_ident.sym)
                        {
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
        self.current_function.mark_has_eval();

        let mut eval_block = block;
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

        let bindings: Vec<_> = self
            .scopes
            .visible_bindings()
            .into_iter()
            .filter(|(_, name, _)| !matches!(name.as_str(), "undefined" | "NaN" | "Infinity"))
            .collect();

        let env_val = self.alloc_value();
        self.current_function.append_instruction(
            eval_block,
            Instruction::NewObject {
                dest: env_val,
                capacity: bindings.len() as u32 + u32::from(self.strict_mode),
            },
        );

        if self.strict_mode {
            let key_const = self
                .module
                .add_constant(Constant::String("__wjsm_eval_strict".to_string()));
            let key_val = self.alloc_value();
            self.current_function.append_instruction(
                eval_block,
                Instruction::Const {
                    dest: key_val,
                    constant: key_const,
                },
            );
            let true_const = self.module.add_constant(Constant::Bool(true));
            let true_val = self.alloc_value();
            self.current_function.append_instruction(
                eval_block,
                Instruction::Const {
                    dest: true_val,
                    constant: true_const,
                },
            );
            self.current_function.append_instruction(
                eval_block,
                Instruction::SetProp {
                    object: env_val,
                    key: key_val,
                    value: true_val,
                },
            );
        }

        for (scope_id, name, _) in &bindings {
            let key_const = self.module.add_constant(Constant::String(name.clone()));
            let key_val = self.alloc_value();
            self.current_function.append_instruction(
                eval_block,
                Instruction::Const {
                    dest: key_val,
                    constant: key_const,
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

            self.current_function.append_instruction(
                eval_block,
                Instruction::SetProp {
                    object: env_val,
                    key: key_val,
                    value,
                },
            );
        }
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            eval_block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::Eval,
                args: vec![code_val, env_val],
            },
        );

        // 异常检查：eval 内部 throw 时 TAG_EXCEPTION 不应作为正常值传播
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

        // 异常路径：解封装异常值并传播（可能被 try/catch 捕获，或 throw）
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

        // 正常路径：将 eval 环境修改的变量写回外层作用域
        for (scope_id, name, _) in &bindings {
            let binding = CapturedBinding::new(name.clone(), *scope_id);
            if !self.binding_belongs_to_current_function(&binding)
                || self.is_shared_binding(&binding)
            {
                continue;
            }

            let key_const = self.module.add_constant(Constant::String(name.clone()));
            let key_val = self.alloc_value();
            self.current_function.append_instruction(
                continue_block,
                Instruction::Const {
                    dest: key_val,
                    constant: key_const,
                },
            );

            let value = self.alloc_value();
            self.current_function.append_instruction(
                continue_block,
                Instruction::GetProp {
                    dest: value,
                    object: env_val,
                    key: key_val,
                },
            );
            self.current_function.append_instruction(
                continue_block,
                Instruction::StoreVar {
                    name: binding.var_ir_name(),
                    value,
                },
            );
        }

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
