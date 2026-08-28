use super::*;

impl Lowerer {
    /// 返回 (promise 值, 延续块)：executor 实参求值可能分叉异常/引入控制流，
    /// 调用方必须在返回的延续块上继续，不能停留在入口块。
    pub(crate) fn lower_new_promise(
        &mut self,
        new_expr: &swc_ast::NewExpr,
        block: BasicBlockId,
    ) -> Result<(ValueId, BasicBlockId), LoweringError> {
        let promise_val = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::NewPromise { dest: promise_val });

        let mut end_block = block;
        if let Some(args) = &new_expr.args
            && let Some(first_arg) = args.first()
        {
            let callback_val =
                self.lower_call_operand_then_continue(&first_arg.expr, &mut end_block)?;

            let resolve_fn = self.alloc_value();
            self.current_function.append_instruction(
                end_block,
                Instruction::CallBuiltin {
                    dest: Some(resolve_fn),
                    builtin: Builtin::PromiseCreateResolveFunction,
                    args: vec![promise_val],
                },
            );

            let reject_fn = self.alloc_value();
            self.current_function.append_instruction(
                end_block,
                Instruction::CallBuiltin {
                    dest: Some(reject_fn),
                    builtin: Builtin::PromiseCreateRejectFunction,
                    args: vec![promise_val],
                },
            );

            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                end_block,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );

            self.current_function.append_instruction(
                end_block,
                Instruction::Call {
                    dest: None,
                    callee: callback_val,
                    this_val: undef_val,
                    args: vec![resolve_fn, reject_fn],
                },
            );
        }

        Ok((promise_val, end_block))
    }

    // ── Identifiers ─────────────────────────────────────────────────────────

    pub(crate) fn lower_host_builtin_call_expr(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
        builtin: Builtin,
    ) -> Result<ValueId, LoweringError> {
        if builtin == Builtin::ConsoleLog && call.args.is_empty() {
            return Err(self.error(call.span, "console.log requires at least 1 argument"));
        }
        let mut call_block = block;
        let (effective_builtin, args) = if builtin == Builtin::MathMax
            && Self::call_args_have_spread(&call.args)
        {
            let (args_array, end_block) = self.lower_call_args_to_array(&call.args, call_block)?;
            call_block = end_block;
            (Builtin::MathMaxArray, vec![args_array])
        } else if Self::call_args_have_spread(&call.args) {
            // `Reflect.has(...arguments)` 等不能把 spread 表达式当成单个 CallBuiltin
            // 实参，否则 `let [target, key] = args` 只收到 arguments 对象并 InternalInvariant。
            return self.lower_host_builtin_spread_apply(call, call_block);
        } else {
            let mut args = Vec::with_capacity(call.args.len().max(1));
            for arg in &call.args {
                // 实参抛出必须中止宿主调用并传播，不得把异常哨兵当作实参值传入。
                let arg_val = self.lower_call_operand_then_continue(&arg.expr, &mut call_block)?;
                args.push(arg_val);
            }
            (builtin, args)
        };

        let dest = self.alloc_value();
        self.current_function.append_instruction(
            call_block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: effective_builtin,
                args,
            },
        );
        if matches!(builtin, Builtin::JsonParse) {
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
            self.expr_merge_block = Some(continue_block);
            return Ok(dest);
        }
        self.expr_merge_block = Some(call_block);
        Ok(dest)
    }

    /// 静态宿主 API 的 spread 调用：物化实参数组后 FuncApply。
    fn lower_host_builtin_spread_apply(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let mut call_block = block;
        let (callee_val, this_val) = self.lower_host_apply_callee(call, &mut call_block)?;
        let (args_array, end_block) = self.lower_call_args_to_array(&call.args, call_block)?;
        call_block = end_block;
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            call_block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::FuncApply,
                args: vec![callee_val, this_val, args_array],
            },
        );
        self.expr_merge_block = Some(call_block);
        Ok(dest)
    }

    fn lower_host_apply_callee(
        &mut self,
        call: &swc_ast::CallExpr,
        block: &mut BasicBlockId,
    ) -> Result<(ValueId, ValueId), LoweringError> {
        let swc_ast::Callee::Expr(expr) = &call.callee else {
            return Err(self.error(call.span, "spread apply requires a callable expression"));
        };
        if let swc_ast::Expr::Member(member_expr) = expr.as_ref() {
            let this_val = self.lower_expr_then_continue(member_expr.obj.as_ref(), block)?;
            let callee_val =
                self.lower_member_expr_from_object(member_expr, this_val, block, false)?;
            return Ok((callee_val, this_val));
        }
        let undef_const = self.module.add_constant(Constant::Undefined);
        let this_val = self.alloc_value();
        self.current_function.append_instruction(
            *block,
            Instruction::Const {
                dest: this_val,
                constant: undef_const,
            },
        );
        let callee_val = self.lower_expr_then_continue(expr, block)?;
        Ok((callee_val, this_val))
    }

    /// 处理动态 import() 调用
    pub(crate) fn lower_dynamic_import_call(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let first_arg = call
            .args
            .first()
            .ok_or_else(|| self.error(call.span, "import() requires a module specifier"))?;

        if call.args.len() > 1 {
            // JSON/import-attributes are outside Task 5; reject options before any
            // static literal fast path can ignore them.
            return Err(self.error(
                call.span,
                "import() currently supports only the module specifier argument",
            ));
        }

        if let Some(specifier) = self.static_dynamic_import_specifier(first_arg.expr.as_ref())
            && let Some(current_module_id) = self.current_module_id
            && let Some(target_id) = self.find_dynamic_import_target(current_module_id, &specifier)
        {
            let dest = self.emit_static_dynamic_import(block, target_id);
            self.expr_merge_block = Some(block);
            return Ok(dest);
        }

        self.emit_runtime_dynamic_import(call, first_arg, block)
    }

    fn static_dynamic_import_specifier(&self, expr: &swc_ast::Expr) -> Option<String> {
        match expr {
            swc_ast::Expr::Lit(swc_ast::Lit::Str(s)) => {
                Some(s.value.to_string_lossy().into_owned())
            }
            swc_ast::Expr::Tpl(tpl) if tpl.exprs.is_empty() => {
                let mut result = String::new();
                for quasi in &tpl.quasis {
                    result.push_str(&quasi.raw);
                }
                Some(result)
            }
            _ => None,
        }
    }

    fn emit_static_dynamic_import(
        &mut self,
        block: BasicBlockId,
        target_id: wjsm_ir::ModuleId,
    ) -> ValueId {
        // 静态字符串且 resolver 已给出 ModuleId 时保留 AOT 快路径。
        let module_id_const = self.module.add_constant(Constant::ModuleId(target_id));
        let module_id_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: module_id_val,
                constant: module_id_const,
            },
        );
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::DynamicImport,
                args: vec![module_id_val],
            },
        );
        dest
    }

    fn emit_runtime_dynamic_import(
        &mut self,
        _call: &swc_ast::CallExpr,
        first_arg: &swc_ast::ExprOrSpread,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // ImportCall（ES §13.3.10.1）步骤 2-3 的 `? Evaluation` / `? GetValue`：
        // specifier 求值抛出同步传播（本地 try/catch 或状态机 rejection 终结器），
        // 不转为返回 promise 的 rejection——仅 ToString(specifier) 起才
        // IfAbruptRejectPromise（由宿主 DynamicImportRuntime 处理）。
        let mut call_block = block;
        let specifier_val =
            self.lower_call_operand_then_continue(&first_arg.expr, &mut call_block)?;
        let normal_promise = self.emit_runtime_dynamic_import_call(call_block, specifier_val);
        self.expr_merge_block = Some(call_block);
        Ok(normal_promise)
    }

    fn emit_runtime_dynamic_import_call(
        &mut self,
        block: BasicBlockId,
        specifier_val: ValueId,
    ) -> ValueId {
        let referrer_val = self.emit_runtime_import_referrer(block);
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::DynamicImportRuntime,
                args: vec![referrer_val, specifier_val],
            },
        );
        dest
    }

    fn emit_runtime_import_referrer(&mut self, block: BasicBlockId) -> ValueId {
        let constant = self
            .current_module_id
            .and_then(|module_id| self.module_metadata.get(&module_id))
            .and_then(|metadata| (!metadata.filename.is_empty()).then(|| metadata.filename.clone()))
            .map(Constant::String)
            .unwrap_or(Constant::Undefined);
        let const_id = self.module.add_constant(constant);
        let value = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: value,
                constant: const_id,
            },
        );
        value
    }

    /// 从 specifier 映射中查找动态 import 目标的 ModuleId
    pub(crate) fn find_dynamic_import_target(
        &self,
        current_module_id: wjsm_ir::ModuleId,
        specifier: &str,
    ) -> Option<wjsm_ir::ModuleId> {
        self.dynamic_import_specifier_map
            .get(&(current_module_id, specifier.to_string()))
            .copied()
    }
}
