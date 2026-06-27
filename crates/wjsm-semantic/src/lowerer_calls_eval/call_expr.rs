use super::*;

impl Lowerer {
    /// 原型方法拦截的公共发射逻辑：把 `obj.method(args...)` 降为
    /// `CallBuiltin(builtin, [this, args...])`，其中 this = obj。
    ///
    /// `lower_call_expr` 中 7+ 个拦截点（String/Array/Object/Number/Boolean/
    /// SharedArrayBuffer/DataView 原型方法）此前各自重复这段「lower obj 为 this →
    /// lower 每个实参 → 追加 CallBuiltin → 返回 dest」样板。抽成单一 helper 后，
    /// 拦截点只保留各自的「模式识别 + receiver guard」判定，发射逻辑集中一处。
    /// Whether `expr` is an unshadowed direct `eval(...)` call (expression form).
    fn is_direct_eval_call_expr(&self, expr: &swc_ast::Expr) -> bool {
        let swc_ast::Expr::Call(call) = expr else {
            return false;
        };
        matches!(
            &call.callee,
            swc_ast::Callee::Expr(callee)
                if matches!(callee.as_ref(), swc_ast::Expr::Ident(ident) if ident.sym.as_ref() == "eval")
        ) && self.scopes.lookup("eval").is_err()
    }
    fn emit_proto_builtin_call(
        &mut self,
        builtin: Builtin,
        obj: &swc_ast::Expr,
        args: &[swc_ast::ExprOrSpread],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let this_val = self.lower_expr(obj, block)?;
        let mut builtin_args = vec![this_val];
        for arg in args {
            builtin_args.push(self.lower_expr(&arg.expr, block)?);
        }
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin,
                args: builtin_args,
            },
        );
        Ok(dest)
    }

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

                if let swc_ast::Expr::SuperProp(super_prop) = expr.as_ref() {
                    this_val = self.lower_this(block)?;
                    callee_val = self.lower_super_prop(super_prop, block)?;
                // 检测 MemberExpr 被调用者 → 提取 obj 作为 this
                } else if let swc_ast::Expr::Member(member_expr) = expr.as_ref() {
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
                        return self.emit_proto_builtin_call(
                            ta_builtin,
                            &member_expr.obj,
                            &call.args,
                            block,
                        );
                    }
                    // SharedArrayBuffer.prototype 方法调用优化（带 receiver guard）。
                    // 必须在 String 之前，以确保 sab.slice 等优先匹配；仅当 obj 是已知 SAB 绑定时才拦截，
                    // 否则回退通用路径，避免劫持 String.prototype.slice / Array 等同名方法（P1 修复）。
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
                        && let Some(sab_builtin) =
                            builtin_from_sharedarraybuffer_proto_method(&prop_ident.sym)
                        && let swc_ast::Expr::Ident(receiver_ident) = member_expr.obj.as_ref()
                        && self.is_sharedarraybuffer_binding(receiver_ident)
                    {
                        return self.emit_proto_builtin_call(
                            sab_builtin,
                            &member_expr.obj,
                            &call.args,
                            block,
                        );
                    }
                    // DataView.prototype get/set 方法使用非 Type 12 的专用宿主导入；
                    // 对静态已知 DataView receiver 直连 CallBuiltin，避免通用 call_indirect 调用约定不匹配。
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
                        && let Some(dv_builtin) =
                            builtin_from_dataview_proto_method(&prop_ident.sym)
                        && let swc_ast::Expr::Ident(receiver_ident) = member_expr.obj.as_ref()
                        && self.is_dataview_binding(receiver_ident)
                    {
                        return self.emit_proto_builtin_call(
                            dv_builtin,
                            &member_expr.obj,
                            &call.args,
                            block,
                        );
                    }
                    // String.prototype 方法调用优化（必须在 Array 之前，因为 at/slice/concat 等方法在 String 和 Array 上同名）
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
                        && let Some(string_builtin) =
                            builtin_from_string_proto_method(&prop_ident.sym)
                    {
                        let _ = builtin_call_signature(string_builtin);
                        return self.emit_proto_builtin_call(
                            string_builtin,
                            &member_expr.obj,
                            &call.args,
                            block,
                        );
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
                            return self.emit_proto_builtin_call(
                                array_builtin,
                                &member_expr.obj,
                                &call.args,
                                block,
                            );
                        }

                        // TypedArray.prototype 方法调用优化：发出 CallBuiltin 代替 Call，
                        // 跳过运行时属性解析。
                        if let Some(ta_builtin) =
                            builtin_from_typedarray_proto_method(&prop_ident.sym)
                            && let swc_ast::Expr::Ident(receiver_ident) = member_expr.obj.as_ref()
                            && self.is_typedarray_binding(receiver_ident)
                        {
                            return self.emit_proto_builtin_call(
                                ta_builtin,
                                &member_expr.obj,
                                &call.args,
                                block,
                            );
                        }

                        // Function.prototype.call/apply/bind: func.call(thisArg, ...args)
                        if let Some(func_builtin) =
                            builtin_from_function_proto_method(&prop_ident.sym)
                        {
                            // 特殊优化: Object.prototype.toString.call(obj) → CallBuiltin(ObjectProtoToString, obj)
                            // 跳过运行时原型链查找
                            if matches!(func_builtin, Builtin::FuncCall)
                                && let Some(object_proto_builtin) =
                                    self.is_object_proto_method_access(&member_expr.obj)
                            {
                                // Object.prototype.toString.call(thisArg) → ObjectProtoToString(thisArg)
                                let this_arg = if let Some(first_arg) = call.args.first() {
                                    self.lower_expr(&first_arg.expr, block)?
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
                            return self.emit_proto_builtin_call(
                                obj_proto_builtin,
                                &member_expr.obj,
                                &call.args,
                                block,
                            );
                        }

                        if let Some(promise_proto_builtin) =
                            builtin_from_promise_proto_method(&prop_ident.sym)
                        {
                            let mut call_block = block;
                            this_val =
                                self.lower_expr_then_continue(&member_expr.obj, &mut call_block)?;
                            let mut builtin_args = vec![this_val];
                            for arg in &call.args {
                                builtin_args.push(
                                    self.lower_expr_then_continue(&arg.expr, &mut call_block)?,
                                );
                            }
                            if builtin_args.len() < 3
                                && matches!(promise_proto_builtin, Builtin::PromiseThen)
                            {
                                let undef_const = self.module.add_constant(Constant::Undefined);
                                let undef_val = self.alloc_value();
                                self.current_function.append_instruction(
                                    call_block,
                                    Instruction::Const {
                                        dest: undef_val,
                                        constant: undef_const,
                                    },
                                );
                                builtin_args.push(undef_val);
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                call_block,
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
                            && self.should_use_number_proto_call_fast_path(
                                prop_ident.sym.as_ref(),
                                member_expr.obj.as_ref(),
                            )
                        {
                            return self.emit_proto_builtin_call(
                                number_proto_builtin,
                                &member_expr.obj,
                                &call.args,
                                block,
                            );
                        }

                        if let Some(boolean_proto_builtin) =
                            builtin_from_boolean_proto_method(&prop_ident.sym)
                        {
                            return self.emit_proto_builtin_call(
                                boolean_proto_builtin,
                                &member_expr.obj,
                                &call.args,
                                block,
                            );
                        }
                    }

                    // obj.method() → obj 是 this，method 是 callee（未被拦截时）。
                    // obj 可能因捕获绑定读取产生分支/phi，后续取属性必须接在继续块上。
                    let mut member_block = block;
                    this_val =
                        self.lower_expr_then_continue(&member_expr.obj, &mut member_block)?;
                    callee_val = self.lower_member_expr_from_object(
                        member_expr,
                        this_val,
                        &mut member_block,
                    )?;
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
            swc_ast::Callee::Super(super_token) => {
                if !self.super_call_allowed {
                    return Err(self.error(
                        super_token.span,
                        "super() is only valid inside derived constructors",
                    ));
                }
                let callee = self.alloc_value();
                self.current_function
                    .append_instruction(block, Instruction::GetSuperConstructor { dest: callee });
                let this_val = self.lower_this(block)?;
                let mut call_block = self.resolve_store_block(block);
                let mut args = Vec::with_capacity(call.args.len());
                for arg in &call.args {
                    args.push(self.lower_expr_then_continue(&arg.expr, &mut call_block)?);
                }
                let ctor_result = self.alloc_value();
                self.current_function.append_instruction(
                    call_block,
                    Instruction::SuperCall {
                        dest: Some(ctor_result),
                        callee,
                        this_val,
                        args,
                        forward_args: false,
                    },
                );
                let (result, _) =
                    self.select_construct_result(call_block, ctor_result, this_val);
                return Ok(result);
            }
        }
        // 性能优化：预分配容量避免循环中多次 reallocation
        let mut call_block = self.resolve_store_block(block);
        let mut args = Vec::with_capacity(call.args.len());
        for arg in &call.args {
            let arg_val = self.lower_expr_then_continue(&arg.expr, &mut call_block)?;
            if self.expr_exception_fork_allowed() && self.is_direct_eval_call_expr(&arg.expr) {
                call_block = self.lower_value_exception_branch(call_block, arg_val)?;
            }
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
        if let Some(first_arg) = call.args.first()
            && self.is_direct_eval_call_expr(&first_arg.expr)
            && self.expr_exception_fork_allowed()
        {
            eval_block = self.lower_value_exception_branch(eval_block, code_val)?;
        }

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

        // 7. Set meta: super base (key=2). 非方法上下文会得到 undefined。
        let super_key = self.const_val_i64(eval_block, 2);
        let super_base = self.alloc_value();
        self.current_function
            .append_instruction(eval_block, Instruction::GetSuperBase { dest: super_base });
        let super_name = self
            .module
            .add_constant(Constant::String("__wjsm_super_base".to_string()));
        let super_name_val = self.alloc_value();
        self.current_function.append_instruction(
            eval_block,
            Instruction::Const {
                dest: super_name_val,
                constant: super_name,
            },
        );
        let super_false = self.const_val_i64(eval_block, 0);
        self.current_function.append_instruction(
            eval_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ScopeRecordAddBinding,
                args: vec![
                    scope_record,
                    super_name_val,
                    super_base,
                    super_false,
                    super_false,
                ],
            },
        );
        self.current_function.append_instruction(
            eval_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ScopeRecordSetMeta,
                args: vec![scope_record, super_key, super_base],
            },
        );
        // 7b. new.target (meta key=3). 箭头函数从词法环境捕获，普通函数读取当前调用上下文。
        let nt_key = self.const_val_i64(eval_block, 3);
        let new_target = if self.is_arrow {
            let binding = CapturedBinding::lexical_new_target();
            self.record_capture(binding.clone());
            let env_val = self.load_env_object(eval_block);
            let key_val = self.append_env_key_const(eval_block, &binding);
            let new_target = self.alloc_value();
            self.current_function.append_instruction(
                eval_block,
                Instruction::GetProp {
                    dest: new_target,
                    object: env_val,
                    key: key_val,
                },
            );
            new_target
        } else {
            let new_target = self.alloc_value();
            let dummy_const = self.module.add_constant(Constant::Undefined);
            let dummy_val = self.alloc_value();
            self.current_function.append_instruction(
                eval_block,
                Instruction::Const {
                    dest: dummy_val,
                    constant: dummy_const,
                },
            );
            self.current_function.append_instruction(
                eval_block,
                Instruction::CallBuiltin {
                    dest: Some(new_target),
                    builtin: Builtin::NewTarget,
                    args: vec![dummy_val],
                },
            );
            new_target
        };
        self.current_function.append_instruction(
            eval_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ScopeRecordSetMeta,
                args: vec![scope_record, nt_key, new_target],
            },
        );
        // new.target for eval body: runtime reads scope meta first, then runtime global fallback.

        // 8. Call Eval(code, scope_record)
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

        // 10. Writeback: read post-eval values for visible bindings (incl. TDZ let/const after assign)
        for (scope_id, name, _, is_initialised) in &all_bindings {
            if !*is_initialised {
                continue;
            }
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

}
