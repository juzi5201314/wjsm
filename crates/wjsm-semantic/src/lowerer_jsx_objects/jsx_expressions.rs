use super::*;

impl Lowerer {
    pub(crate) fn lower_expr(
        &mut self,
        expr: &swc_ast::Expr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        match expr {
            swc_ast::Expr::Bin(bin) => self.lower_binary(bin, block),
            swc_ast::Expr::Lit(lit) => self.lower_literal(lit, block),
            swc_ast::Expr::Ident(ident) => self.lower_ident(ident, block),
            swc_ast::Expr::Assign(assign) => self.lower_assign(assign, block),
            swc_ast::Expr::Unary(unary) => self.lower_unary(unary, block),
            swc_ast::Expr::Cond(cond) => self.lower_cond(cond, block),
            swc_ast::Expr::Seq(seq) => self.lower_seq(seq, block),
            swc_ast::Expr::Paren(paren) => self.lower_expr(&paren.expr, block),
            swc_ast::Expr::OptChain(oc) => self.lower_optchain(oc, block),
            swc_ast::Expr::Call(call) => self.lower_call_expr(call, block),
            swc_ast::Expr::Fn(fn_expr) => self.lower_fn_expr(fn_expr, block),
            swc_ast::Expr::Arrow(arrow) => self.lower_arrow_expr(arrow, block),
            swc_ast::Expr::Object(obj_expr) => self.lower_object_expr(obj_expr, block),
            swc_ast::Expr::Array(arr) => self.lower_array_expr(arr, block),
            swc_ast::Expr::Member(member) => self.lower_member_expr(member, block),
            swc_ast::Expr::This(_) => self.lower_this(block),
            swc_ast::Expr::New(new_expr) => {
                let (val, new_block) = self.lower_new_expr(new_expr, block)?;
                self.new_expr_continue_block = Some(new_block);
                Ok(val)
            }
            swc_ast::Expr::Class(class_expr) => self.lower_class_expr(class_expr, block),
            swc_ast::Expr::Update(update) => self.lower_update(update, block),
            swc_ast::Expr::Tpl(tpl) => self.lower_tpl(tpl, block),
            swc_ast::Expr::TaggedTpl(tagged_tpl) => self.lower_tagged_tpl(tagged_tpl, block),
            swc_ast::Expr::SuperProp(super_prop) => self.lower_super_prop(super_prop, block),
            swc_ast::Expr::Await(await_expr) => {
                if !self.is_async_fn {
                    return Err(self.error(expr.span(), "await is only valid in async functions"));
                }
                self.lower_await_expr(await_expr, block)
            }
            swc_ast::Expr::Yield(yield_expr) => self.lower_yield_expr(yield_expr, block),
            // TS type assertion expressions — 编译时类型信息，透传内层表达式
            swc_ast::Expr::TsTypeAssertion(ts_assert) => self.lower_expr(&ts_assert.expr, block),
            swc_ast::Expr::TsConstAssertion(assert) => self.lower_expr(&assert.expr, block),
            swc_ast::Expr::TsNonNull(ts_non_null) => self.lower_expr(&ts_non_null.expr, block),
            swc_ast::Expr::TsAs(ts_as) => self.lower_expr(&ts_as.expr, block),
            swc_ast::Expr::TsSatisfies(ts_satisfies) => self.lower_expr(&ts_satisfies.expr, block),
            swc_ast::Expr::TsInstantiation(ts_inst) => self.lower_expr(&ts_inst.expr, block),
            // JSX expressions
            swc_ast::Expr::JSXElement(jsx_el) => self.lower_jsx_element(jsx_el, block),
            swc_ast::Expr::JSXFragment(jsx_frag) => self.lower_jsx_fragment(jsx_frag, block),
            swc_ast::Expr::JSXEmpty(_) => {
                let null_const = self.module.add_constant(Constant::Null);
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest,
                        constant: null_const,
                    },
                );
                Ok(dest)
            }
            swc_ast::Expr::MetaProp(meta) => match meta.kind {
                swc_ast::MetaPropKind::NewTarget => {
                    // new.target is only valid in function code (not top-level)
                    if self.function_stack.is_empty() {
                        if self.eval_scope_record {
                            // Read new.target from scope record
                            let env = self.load_eval_scope_env(block);
                            let name_const = self
                                .module
                                .add_constant(Constant::String("__wjsm_new_target".to_string()));
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
                            return Ok(dest);
                        } else if self.eval_mode {
                            // eval at top-level without scope record: new.target is a SyntaxError
                            return Err(self.error(
                                meta.span,
                                "SyntaxError: new.target expression is not valid in top-level eval",
                            ));
                        } else {
                            return Err(self.error(
                                meta.span,
                                "SyntaxError: new.target expression is not valid outside functions",
                            ));
                        }
                    }
                    // In eval mode: only valid if eval is inside a non-arrow function
                    if self.eval_mode
                        && !self.function_stack.is_empty()
                        && self.is_arrow_fn_stack.last().copied() == Some(true)
                    {
                        return Err(self.error(
                            meta.span,
                            "SyntaxError: new.target is not valid in arrow function eval",
                        ));
                    }
                    if self.eval_scope_record {
                        let env = self.load_eval_scope_env(block);
                        let name_const = self
                            .module
                            .add_constant(Constant::String("__wjsm_new_target".to_string()));
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
                        return Ok(dest);
                    }

                    let dest = self.alloc_value();
                    // dummy 参数是 NewTarget host builtin 的签名要求（expects 1 dummy arg），移除会导致 runtime error。
                    // 真正的 '0' / mismatch 问题在 Construct/Call 时的 NewTargetSet 动态状态或 initial value，需在 compiler_instructions 修复。
                    let dummy_const = self.module.add_constant(Constant::Undefined);
                    let dummy_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: dummy_val,
                            constant: dummy_const,
                        },
                    );
                    self.current_function.append_instruction(
                        block,
                        Instruction::CallBuiltin {
                            dest: Some(dest),
                            builtin: Builtin::NewTarget,
                            args: vec![dummy_val],
                        },
                    );
                    Ok(dest)
                }
                swc_ast::MetaPropKind::ImportMeta => {
                    Err(self.error(meta.span, "SyntaxError: import.meta is not supported"))
                }
            },
            _ => Err(self.error(
                expr.span(),
                format!("unsupported expression kind `{}`", expr_kind(expr)),
            )),
        }
    }

    pub(crate) fn lower_tpl(
        &mut self,
        tpl: &swc_ast::Tpl,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // quasis (静态文本段) + exprs (动态表达式) 交错: quasi[0] expr[0] quasi[1] expr[1] ... quasi[n]
        let mut parts = Vec::with_capacity(tpl.quasis.len() + tpl.exprs.len());
        for (i, quasi) in tpl.quasis.iter().enumerate() {
            let cooked_str = quasi.cooked.as_ref().ok_or_else(|| {
                self.error(
                    quasi.span,
                    "template string quasi has no cooked value".to_string(),
                )
            })?;
            let cooked_s = cooked_str.to_atom_lossy().to_string();
            let const_id = self.module.add_constant(Constant::String(cooked_s));
            let val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: val,
                    constant: const_id,
                },
            );
            parts.push(val);
            if i < tpl.exprs.len() {
                let expr_val = self.lower_expr(&tpl.exprs[i], block)?;
                parts.push(expr_val);
            }
        }
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::StringConcatVa { dest, parts });
        Ok(dest)
    }

    pub(crate) fn lower_tagged_tpl(
        &mut self,
        tagged_tpl: &swc_ast::TaggedTpl,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let tpl = &tagged_tpl.tpl;
        // 1. 构建 cooked quasi 数组
        let cooked_arr = self.lower_quasis_to_array(tpl, block, false)?;
        // 2. 构建 raw quasi 数组
        let raw_arr = self.lower_quasis_to_array(tpl, block, true)?;
        // 3. Object.defineProperty(cooked_arr, "raw", { value: raw_arr, ... })
        let define_prop_const = self
            .module
            .add_constant(Constant::String("raw".to_string()));
        let define_prop_key = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: define_prop_key,
                constant: define_prop_const,
            },
        );
        // 描述符对象: { value: raw_arr, writable: false, enumerable: false, configurable: false }
        let desc_obj_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: desc_obj_val,
                capacity: 4,
            },
        );
        // value
        let value_key = self
            .module
            .add_constant(Constant::String("value".to_string()));
        let value_key_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: value_key_val,
                constant: value_key,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: desc_obj_val,
                key: value_key_val,
                value: raw_arr,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::DefineProperty,
                args: vec![cooked_arr, define_prop_key, desc_obj_val],
            },
        );
        // 4. 解析 callee + this_val（复用 lower_call_expr 的逻辑）
        let (callee_val, this_val) = self.lower_tag_expr(&tagged_tpl.tag, block)?;
        // 5. 收集参数: [cooked_arr, ...exprs]
        let mut args = Vec::with_capacity(1 + tpl.exprs.len());
        args.push(cooked_arr);
        for expr in &tpl.exprs {
            let expr_val = self.lower_expr(expr, block)?;
            args.push(expr_val);
        }
        // 6. 发出 Call 指令
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Call {
                dest: Some(dest),
                callee: callee_val,
                this_val,
                args,
            },
        );
        Ok(dest)
    }

    pub(crate) fn lower_quasis_to_array(
        &mut self,
        tpl: &swc_ast::Tpl,
        block: BasicBlockId,
        raw: bool,
    ) -> Result<ValueId, LoweringError> {
        let arr = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewArray {
                dest: arr,
                capacity: tpl.quasis.len() as u32,
            },
        );
        for quasi in &tpl.quasis {
            let s = if raw {
                quasi.raw.as_str().to_string()
            } else {
                let cooked = quasi.cooked.as_ref().ok_or_else(|| {
                    self.error(
                        quasi.span,
                        "template string quasi has no cooked value".to_string(),
                    )
                })?;
                cooked.to_atom_lossy().to_string()
            };
            let const_id = self.module.add_constant(Constant::String(s));
            let val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: val,
                    constant: const_id,
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ArrayPush,
                    args: vec![arr, val],
                },
            );
        }
        Ok(arr)
    }

    /// 解析 tagged template 的 tag 表达式，返回 (callee, this_val)。
    /// 复用 lower_call_expr 的 MemberExpression 解析逻辑。
    pub(crate) fn lower_tag_expr(
        &mut self,
        expr: &swc_ast::Expr,
        block: BasicBlockId,
    ) -> Result<(ValueId, ValueId), LoweringError> {
        match expr {
            swc_ast::Expr::Member(member_expr) => {
                let this_val = self.lower_expr(&member_expr.obj, block)?;
                let callee_val = self.lower_member_expr(member_expr, block)?;
                Ok((callee_val, this_val))
            }
            _ => {
                let undef_const = self.module.add_constant(Constant::Undefined);
                let this_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: this_val,
                        constant: undef_const,
                    },
                );
                let callee_val = self.lower_expr(expr, block)?;
                Ok((callee_val, this_val))
            }
        }
    }

    fn lower_optchain(
        &mut self,
        oc: &swc_ast::OptChainExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        match oc.base.as_ref() {
            swc_ast::OptChainBase::Member(member) => self.lower_member_expr(member, block),
            swc_ast::OptChainBase::Call(ocall) => {
                let call_expr = swc_ast::CallExpr {
                    span: ocall.span,
                    ctxt: ocall.ctxt,
                    callee: swc_ast::Callee::Expr(ocall.callee.clone()),
                    args: ocall.args.to_vec(),
                    type_args: ocall.type_args.clone(),
                };
                self.lower_call_expr(&call_expr, block)
            }
        }
    }
}
