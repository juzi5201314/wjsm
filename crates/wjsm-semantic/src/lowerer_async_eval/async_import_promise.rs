use super::*;

impl Lowerer {
    pub(crate) fn lower_new_promise(
        &mut self,
        new_expr: &swc_ast::NewExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let promise_val = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::NewPromise { dest: promise_val });

        if let Some(args) = &new_expr.args
            && let Some(first_arg) = args.first()
        {
            let mut call_block = block;
            let callback_val = self.lower_expr_then_continue(&first_arg.expr, &mut call_block)?;

            let resolve_fn = self.alloc_value();
            self.current_function.append_instruction(
                call_block,
                Instruction::CallBuiltin {
                    dest: Some(resolve_fn),
                    builtin: Builtin::PromiseCreateResolveFunction,
                    args: vec![promise_val],
                },
            );

            let reject_fn = self.alloc_value();
            self.current_function.append_instruction(
                call_block,
                Instruction::CallBuiltin {
                    dest: Some(reject_fn),
                    builtin: Builtin::PromiseCreateRejectFunction,
                    args: vec![promise_val],
                },
            );

            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                call_block,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );

            self.current_function.append_instruction(
                call_block,
                Instruction::Call {
                    dest: None,
                    callee: callback_val,
                    this_val: undef_val,
                    args: vec![resolve_fn, reject_fn],
                },
            );
        }

        Ok(promise_val)
    }

    // ── Identifiers ─────────────────────────────────────────────────────────

    pub(crate) fn lower_host_builtin_call_expr(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
        builtin: Builtin,
    ) -> Result<ValueId, LoweringError> {
        let (name, min_args) = builtin_call_signature(builtin);
        if call.args.len() < min_args {
            return Err(self.error(
                call.span(),
                format!("{name} requires at least {min_args} argument"),
            ));
        }

        let mut args = Vec::with_capacity(call.args.len().max(1));
        let mut call_block = block;
        for arg in &call.args {
            let arg_val = self.lower_expr_then_continue(&arg.expr, &mut call_block)?;
            args.push(arg_val);
        }

        let dest = self.alloc_value();
        self.current_function.append_instruction(
            call_block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin,
                args,
            },
        );
        if matches!(builtin, Builtin::JsonParse) && !self.exception_fork_suppressed() {
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
        let mut call_block = block;
        let (specifier_val, abrupt_specifiers) = self
            .lower_expr_collecting_exception_forks_then_continue(
                &first_arg.expr,
                &mut call_block,
            )?;
        let normal_promise = self.emit_runtime_dynamic_import_call(call_block, specifier_val);

        if abrupt_specifiers.is_empty() {
            self.expr_merge_block = Some(call_block);
            return Ok(normal_promise);
        }

        let merge_block = self.current_function.new_block();
        self.current_function.set_terminator(
            call_block,
            Terminator::Jump {
                target: merge_block,
            },
        );
        let mut sources = Vec::with_capacity(abrupt_specifiers.len() + 1);
        sources.push(PhiSource {
            predecessor: call_block,
            value: normal_promise,
        });

        for (abrupt_block, abrupt_specifier) in abrupt_specifiers {
            let promise = self.emit_runtime_dynamic_import_call(abrupt_block, abrupt_specifier);
            self.current_function.set_terminator(
                abrupt_block,
                Terminator::Jump {
                    target: merge_block,
                },
            );
            sources.push(PhiSource {
                predecessor: abrupt_block,
                value: promise,
            });
        }

        let dest = self.alloc_value();
        self.current_function
            .append_instruction(merge_block, Instruction::Phi { dest, sources });
        self.expr_merge_block = Some(merge_block);
        Ok(dest)
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
