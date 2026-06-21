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

    /// 处理动态 import() 调用
    pub(crate) fn lower_dynamic_import_call(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 1. 提取 specifier 字符串
        let first_arg = call.args.first().ok_or_else(|| {
            self.error(
                call.span,
                "import() requires a module specifier; \
                 in AOT compilation mode, only string literal specifiers are supported",
            )
        })?;

        let specifier = match first_arg.expr.as_ref() {
            swc_ast::Expr::Lit(swc_ast::Lit::Str(s)) => s.value.to_string_lossy().into_owned(),
            swc_ast::Expr::Tpl(tpl) => {
                if tpl.exprs.is_empty() {
                    let mut result = String::new();
                    for quasi in &tpl.quasis {
                        result.push_str(&quasi.raw);
                    }
                    result
                } else {
                    return Err(self.error(
                        call.span,
                        "import() with template literal containing expressions is not supported; \
                         AOT compilation requires the specifier to be a static string literal",
                    ));
                }
            }
            _ => {
                return Err(self.error(
                    call.span,
                    "import() requires a string literal specifier; \
                     AOT compilation cannot resolve dynamic specifiers at compile time. \
                     Use a string literal like import('./module.js') instead",
                ));
            }
        };

        // 2. 查找目标模块 ID
        let current_module_id = self.current_module_id.ok_or_else(|| {
            self.error(
                call.span,
                "dynamic import is only supported in multi-module mode",
            )
        })?;

        let target_id = self
            .find_dynamic_import_target(current_module_id, &specifier)
            .ok_or_else(|| {
                self.error(
                    call.span,
                    format!("cannot resolve dynamic import specifier '{}'", specifier),
                )
            })?;

        // 3. 生成 CallBuiltin(DynamicImport, [module_id])
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
        Ok(dest)
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
