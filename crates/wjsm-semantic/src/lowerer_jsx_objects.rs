use super::*;
use swc_core::ecma::visit::{Visit, VisitWith};

#[derive(Default)]
struct ObjectMethodHomeUse {
    needs_home: bool,
}

impl Visit for ObjectMethodHomeUse {
    fn visit_super_prop_expr(&mut self, _: &swc_ast::SuperPropExpr) {
        self.needs_home = true;
    }

    fn visit_call_expr(&mut self, call: &swc_ast::CallExpr) {
        if let swc_ast::Callee::Expr(callee) = &call.callee
            && let swc_ast::Expr::Ident(ident) = callee.as_ref()
            && ident.sym.as_ref() == "eval"
        {
            self.needs_home = true;
        }
        call.visit_children_with(self);
    }

    fn visit_function(&mut self, _: &swc_ast::Function) {}

    fn visit_class(&mut self, _: &swc_ast::Class) {}
}

fn block_needs_home_object(block: &swc_ast::BlockStmt) -> bool {
    let mut visitor = ObjectMethodHomeUse::default();
    block.visit_with(&mut visitor);
    visitor.needs_home
}

impl Lowerer {
    pub(crate) fn lower_jsx_element(
        &mut self,
        jsx_el: &swc_ast::JSXElement,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 降低 tag 名
        let tag_val = self.lower_jsx_element_name(&jsx_el.opening.name, block)?;

        // 降低 props
        let props_val = self.lower_jsx_attrs(&jsx_el.opening.attrs, block)?;

        // 降低 children（作为数组）
        let children_val = self.lower_jsx_children(&jsx_el.children, block)?;

        // 调用 jsx_create_element(tag, props, children)
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::JsxCreateElement,
                args: vec![tag_val, props_val, children_val],
            },
        );
        Ok(dest)
    }

    pub(crate) fn lower_jsx_fragment(
        &mut self,
        jsx_frag: &swc_ast::JSXFragment,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // Fragment 使用字符串标记 "$JsxFragment"
        let tag_str = "$JsxFragment".to_string();
        let tag_const = self.module.add_constant(Constant::String(tag_str));
        let tag_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: tag_val,
                constant: tag_const,
            },
        );

        // Fragment 的 props 为 null
        let null_const = self.module.add_constant(Constant::Null);
        let props_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: props_val,
                constant: null_const,
            },
        );

        // 收集 children
        let children_val = self.lower_jsx_children(&jsx_frag.children, block)?;

        // 调用 jsx_create_element(tag, null, children)
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::JsxCreateElement,
                args: vec![tag_val, props_val, children_val],
            },
        );
        Ok(dest)
    }

    pub(crate) fn lower_jsx_element_name(
        &mut self,
        name: &swc_ast::JSXElementName,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        match name {
            swc_ast::JSXElementName::Ident(ident) => {
                // HTML 标签名 → 字符串常量
                let tag_str = ident.sym.to_string();
                let tag_const = self.module.add_constant(Constant::String(tag_str));
                let tag_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: tag_val,
                        constant: tag_const,
                    },
                );
                Ok(tag_val)
            }
            swc_ast::JSXElementName::JSXMemberExpr(member_expr) => {
                // <Foo.Bar /> → 降低为成员表达式
                let obj_val = self.lower_jsx_object(&member_expr.obj, block)?;
                let prop_name = member_expr.prop.sym.to_string();
                let prop_const = self.module.add_constant(Constant::String(prop_name));
                let prop_key = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: prop_key,
                        constant: prop_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest,
                        object: obj_val,
                        key: prop_key,
                    },
                );
                Ok(dest)
            }
            swc_ast::JSXElementName::JSXNamespacedName(ns_name) => {
                // <ns:tag /> → 字符串 "ns:tag"
                let tag_str = format!("{}:{}", ns_name.ns.sym, ns_name.name.sym);
                let tag_const = self.module.add_constant(Constant::String(tag_str));
                let tag_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: tag_val,
                        constant: tag_const,
                    },
                );
                Ok(tag_val)
            }
        }
    }

    pub(crate) fn lower_jsx_object(
        &mut self,
        obj: &swc_ast::JSXObject,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        match obj {
            swc_ast::JSXObject::JSXMemberExpr(member_expr) => {
                let obj_val = self.lower_jsx_object(&member_expr.obj, block)?;
                let prop_name = member_expr.prop.sym.to_string();
                let prop_const = self.module.add_constant(Constant::String(prop_name));
                let prop_key = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: prop_key,
                        constant: prop_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest,
                        object: obj_val,
                        key: prop_key,
                    },
                );
                Ok(dest)
            }
            swc_ast::JSXObject::Ident(ident) => self.lower_ident(ident, block),
        }
    }

    pub(crate) fn lower_jsx_attrs(
        &mut self,
        attrs: &[swc_ast::JSXAttrOrSpread],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if attrs.is_empty() {
            // 无属性 → null
            let null_const = self.module.add_constant(Constant::Null);
            let null_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: null_val,
                    constant: null_const,
                },
            );
            return Ok(null_val);
        }

        // 创建 props 对象
        let capacity = std::cmp::max(4, attrs.len() as u32);
        let obj_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: obj_dest,
                capacity,
            },
        );

        for attr_or_spread in attrs {
            match attr_or_spread {
                swc_ast::JSXAttrOrSpread::JSXAttr(attr) => {
                    let attr_name = match &attr.name {
                        swc_ast::JSXAttrName::Ident(ident) => ident.sym.to_string(),
                        swc_ast::JSXAttrName::JSXNamespacedName(ns_name) => {
                            format!("{}:{}", ns_name.ns.sym, ns_name.name.sym)
                        }
                    };

                    let attr_value = if let Some(ref value) = attr.value {
                        match value {
                            swc_ast::JSXAttrValue::Str(s) => {
                                let str_val = s.value.to_string_lossy().into_owned();
                                let const_id = self.module.add_constant(Constant::String(str_val));
                                let val = self.alloc_value();
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::Const {
                                        dest: val,
                                        constant: const_id,
                                    },
                                );
                                val
                            }
                            swc_ast::JSXAttrValue::JSXExprContainer(expr_container) => {
                                match &expr_container.expr {
                                    swc_ast::JSXExpr::Expr(expr) => self.lower_expr(expr, block)?,
                                    swc_ast::JSXExpr::JSXEmptyExpr(_) => {
                                        // 空表达式 → true
                                        let true_const =
                                            self.module.add_constant(Constant::Bool(true));
                                        let val = self.alloc_value();
                                        self.current_function.append_instruction(
                                            block,
                                            Instruction::Const {
                                                dest: val,
                                                constant: true_const,
                                            },
                                        );
                                        val
                                    }
                                }
                            }
                            swc_ast::JSXAttrValue::JSXElement(el) => {
                                self.lower_jsx_element(el, block)?
                            }
                            swc_ast::JSXAttrValue::JSXFragment(frag) => {
                                self.lower_jsx_fragment(frag, block)?
                            }
                        }
                    } else {
                        // 无值属性（如 <input disabled />）→ true
                        let true_const = self.module.add_constant(Constant::Bool(true));
                        let val = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: val,
                                constant: true_const,
                            },
                        );
                        val
                    };

                    // SetProp(obj, attr_name, attr_value)
                    let key_const = self.module.add_constant(Constant::String(attr_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: key_dest,
                            constant: key_const,
                        },
                    );
                    self.current_function.append_instruction(
                        block,
                        Instruction::SetProp {
                            object: obj_dest,
                            key: key_dest,
                            value: attr_value,
                        },
                    );
                }
                swc_ast::JSXAttrOrSpread::SpreadElement(spread) => {
                    let source = self.lower_expr(&spread.expr, block)?;
                    self.current_function.append_instruction(
                        block,
                        Instruction::ObjectSpread {
                            dest: obj_dest,
                            source,
                        },
                    );
                }
            }
        }

        Ok(obj_dest)
    }

    pub(crate) fn lower_jsx_children(
        &mut self,
        children: &[swc_ast::JSXElementChild],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if children.is_empty() {
            // 无 children → null
            let null_const = self.module.add_constant(Constant::Null);
            let null_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: null_val,
                    constant: null_const,
                },
            );
            return Ok(null_val);
        }

        // 创建 children 数组
        let arr = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewArray {
                dest: arr,
                capacity: children.len() as u32,
            },
        );

        for child in children {
            let child_val = match child {
                swc_ast::JSXElementChild::JSXText(text) => {
                    let trimmed = text.value.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let str_const = self
                        .module
                        .add_constant(Constant::String(trimmed.to_string()));
                    let val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: val,
                            constant: str_const,
                        },
                    );
                    val
                }
                swc_ast::JSXElementChild::JSXExprContainer(expr_container) => {
                    match &expr_container.expr {
                        swc_ast::JSXExpr::Expr(expr) => self.lower_expr(expr, block)?,
                        swc_ast::JSXExpr::JSXEmptyExpr(_) => continue,
                    }
                }
                swc_ast::JSXElementChild::JSXElement(el) => self.lower_jsx_element(el, block)?,
                swc_ast::JSXElementChild::JSXFragment(frag) => {
                    self.lower_jsx_fragment(frag, block)?
                }
                _ => continue,
            };
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ArrayPush,
                    args: vec![arr, child_val],
                },
            );
        }

        Ok(arr)
    }

    // ── Expressions ─────────────────────────────────────────────────────────

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
                    let dest = self.alloc_value();
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

    pub(crate) fn lower_object_expr(
        &mut self,
        obj_expr: &swc_ast::ObjectLit,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let obj_dest = self.alloc_value();
        // 容量取 4 和属性数量的较大值，确保对象字面量有足够的槽位
        let capacity = std::cmp::max(4, obj_expr.props.len() as u32);
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: obj_dest,
                capacity,
            },
        );

        for prop in &obj_expr.props {
            match prop {
                swc_ast::PropOrSpread::Prop(prop) => match prop.as_ref() {
                    swc_ast::Prop::KeyValue(kv) => {
                        let val_dest = self.lower_expr(&kv.value, block)?;
                        self.lower_object_prop(obj_dest, &kv.key, val_dest, block)?;
                    }
                    swc_ast::Prop::Shorthand(ident) => {
                        let val_dest = self.lower_ident(ident, block)?;
                        let key_str = ident.sym.to_string();
                        let key_const = self.module.add_constant(Constant::String(key_str));
                        let key_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: key_dest,
                                constant: key_const,
                            },
                        );
                        self.current_function.append_instruction(
                            block,
                            Instruction::SetProp {
                                object: obj_dest,
                                key: key_dest,
                                value: val_dest,
                            },
                        );
                    }
                    swc_ast::Prop::Getter(getter) => {
                        let key_dest = self.lower_prop_name(&getter.key, block)?;
                        let body = getter
                            .body
                            .as_ref()
                            .ok_or_else(|| self.error(getter.span, "getter must have a body"))?;
                        let home_object = if block_needs_home_object(body) {
                            Some(obj_dest)
                        } else {
                            None
                        };
                        let fn_value =
                            self.lower_method_to_fn(&getter.key, body, None, home_object, block)?;
                        let desc = self.build_descriptor("get", fn_value, true, true, block)?;
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: None,
                                builtin: Builtin::DefineProperty,
                                args: vec![obj_dest, key_dest, desc],
                            },
                        );
                    }
                    swc_ast::Prop::Setter(setter) => {
                        let key_dest = self.lower_prop_name(&setter.key, block)?;
                        let body = setter
                            .body
                            .as_ref()
                            .ok_or_else(|| self.error(setter.span, "setter must have a body"))?;
                        let home_object = if block_needs_home_object(body) {
                            Some(obj_dest)
                        } else {
                            None
                        };
                        let fn_value = self.lower_method_to_fn(
                            &setter.key,
                            body,
                            Some(true),
                            home_object,
                            block,
                        )?;
                        let desc = self.build_descriptor("set", fn_value, true, true, block)?;
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: None,
                                builtin: Builtin::DefineProperty,
                                args: vec![obj_dest, key_dest, desc],
                            },
                        );
                    }
                    swc_ast::Prop::Method(method) => {
                        let key_dest = self.lower_prop_name(&method.key, block)?;
                        let home_object = if method
                            .function
                            .body
                            .as_ref()
                            .is_some_and(block_needs_home_object)
                        {
                            Some(obj_dest)
                        } else {
                            None
                        };
                        let fn_value = self.lower_method_prop_to_fn(
                            &method.key,
                            &method.function,
                            home_object,
                            block,
                        )?;
                        self.current_function.append_instruction(
                            block,
                            Instruction::SetProp {
                                object: obj_dest,
                                key: key_dest,
                                value: fn_value,
                            },
                        );
                    }
                    _ => {
                        return Err(
                            self.error(prop.span(), "unsupported property kind in object literal")
                        );
                    }
                },
                swc_ast::PropOrSpread::Spread(spread) => {
                    let source = self.lower_expr(&spread.expr, block)?;
                    self.current_function.append_instruction(
                        block,
                        Instruction::ObjectSpread {
                            dest: obj_dest,
                            source,
                        },
                    );
                }
            }
        }

        Ok(obj_dest)
    }

    /// 将 PropName 转换为运行时的 key value：静态名生成 String 常量，Computed 则 lower 表达式
    pub(crate) fn lower_prop_name(
        &mut self,
        key: &swc_ast::PropName,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        match key {
            swc_ast::PropName::Ident(ident) => {
                let key_str = ident.sym.to_string();
                let key_const = self.module.add_constant(Constant::String(key_str));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                Ok(key_dest)
            }
            swc_ast::PropName::Str(s) => {
                let key_str = s.value.to_string_lossy().into_owned();
                let key_const = self.module.add_constant(Constant::String(key_str));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                Ok(key_dest)
            }
            swc_ast::PropName::Computed(computed) => self.lower_expr(&computed.expr, block),
            _ => Err(self.error(key.span(), "unsupported property key kind")),
        }
    }

    /// 对对象字面量中的 KeyValue prop 设置属性，支持计算属性名
    pub(crate) fn lower_object_prop(
        &mut self,
        obj_dest: ValueId,
        key: &swc_ast::PropName,
        val_dest: ValueId,
        block: BasicBlockId,
    ) -> Result<(), LoweringError> {
        let is_proto_key = match key {
            swc_ast::PropName::Ident(ident) => ident.sym.as_ref() == "__proto__",
            swc_ast::PropName::Str(s) => s.value.to_string_lossy().as_ref() == "__proto__",
            _ => false,
        };
        if is_proto_key {
            self.current_function.append_instruction(
                block,
                Instruction::SetProto {
                    object: obj_dest,
                    value: val_dest,
                },
            );
        } else {
            let key_dest = self.lower_prop_name(key, block)?;
            self.current_function.append_instruction(
                block,
                Instruction::SetProp {
                    object: obj_dest,
                    key: key_dest,
                    value: val_dest,
                },
            );
        }
        Ok(())
    }

    fn create_method_env_with_home(
        &mut self,
        block: BasicBlockId,
        captured: &[CapturedBinding],
        home_object: ValueId,
    ) -> ValueId {
        let env_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: env_val,
                capacity: captured.len() as u32 + 1,
            },
        );

        for binding in captured {
            let current_val = if self.binding_belongs_to_current_function(binding) {
                let current_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: current_val,
                        name: binding.var_ir_name(),
                    },
                );
                current_val
            } else {
                self.record_capture(binding.clone());
                let parent_env = self.load_env_object(block);
                let parent_key = self.append_env_key_const(block, binding);
                let current_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest: current_val,
                        object: parent_env,
                        key: parent_key,
                    },
                );
                current_val
            };

            let key_val = self.append_env_key_const(block, binding);
            self.current_function.append_instruction(
                block,
                Instruction::SetProp {
                    object: env_val,
                    key: key_val,
                    value: current_val,
                },
            );
        }

        let home_key = self
            .module
            .add_constant(Constant::String("home".to_string()));
        let home_key_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: home_key_val,
                constant: home_key,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: env_val,
                key: home_key_val,
                value: home_object,
            },
        );

        env_val
    }

    /// 将 getter/setter 方法体编译为内联函数，返回 FunctionRef 的 ValueId
    pub(crate) fn lower_method_to_fn(
        &mut self,
        key: &swc_ast::PropName,
        body: &swc_ast::BlockStmt,
        _is_setter: Option<bool>,
        home_object: Option<ValueId>,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let method_name = match key {
            swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
            swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
            _ => "anonymous".to_string(),
        };
        let fn_name = format!("$0.{method_name}");

        // 推入新的函数上下文（使用 push_function_context 管理作用域栈）
        self.push_function_context(&fn_name, BasicBlockId(0));
        self.super_allowed = home_object.is_some();

        // 声明 $env 和 $this
        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(key.span(), msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(key.span(), msg))?;

        let method_param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        // 预声明提升变量
        self.predeclare_block_stmts(&body.stmts)?;

        let m_entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(m_entry);

        let m_entry = self.emit_arguments_init(m_entry)?;

        // 降低方法体
        let mut m_flow = StmtFlow::Open(m_entry);
        for stmt in &body.stmts {
            if matches!(m_flow, StmtFlow::Terminated) {
                continue;
            }
            m_flow = self.lower_stmt(stmt, m_flow)?;
        }

        if let StmtFlow::Open(b) = m_flow {
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        // Finalize method function
        let m_old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let m_has_eval = m_old_fn.has_eval();
        let m_blocks = m_old_fn.into_blocks();
        let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
        m_ir_function.set_has_eval(m_has_eval);
        m_ir_function.set_params(method_param_ir_names);
        let m_captured = self.captured_names_stack.last().unwrap().clone();
        m_ir_function.set_captured_names(Self::captured_display_names(&m_captured));
        for b in m_blocks {
            m_ir_function.push_block(b);
        }
        let m_function_id = self.module.push_function(m_ir_function);

        self.pop_function_context();

        let m_ref_const = self
            .module
            .add_constant(Constant::FunctionRef(m_function_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: func_ref_val,
                constant: m_ref_const,
            },
        );

        if let Some(home_object) = home_object {
            let env_val = self.create_method_env_with_home(block, &m_captured, home_object);
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            Ok(closure_val)
        } else if m_captured.is_empty() {
            Ok(func_ref_val)
        } else {
            let env_val = self.ensure_shared_env(block, &m_captured, key.span())?;
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            Ok(closure_val)
        }
    }

    // TODO: lower_method_prop_to_fn 与 lower_fn_expr 的逻辑高度相似
    // （创建函数上下文、声明 $env/$this、构建参数、降低函数体、创建闭包等），
    // 未来应提取共享逻辑以减少代码重复。
    pub(crate) fn lower_method_prop_to_fn(
        &mut self,
        key: &swc_ast::PropName,
        function: &swc_ast::Function,
        home_object: Option<ValueId>,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let method_name = match key {
            swc_ast::PropName::Ident(ident) => ident.sym.to_string(),
            swc_ast::PropName::Str(s) => s.value.to_string_lossy().into_owned(),
            _ => "anonymous".to_string(),
        };
        let fn_name = format!("$0.{method_name}");

        self.push_function_context(&fn_name, BasicBlockId(0));
        self.super_allowed = home_object.is_some();

        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(key.span(), msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(key.span(), msg))?;

        let param_ir_names =
            self.build_param_ir_names(&function.params, env_scope_id, this_scope_id)?;

        if let Some(body) = &function.body {
            self.predeclare_block_stmts(&body.stmts)?;
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let body_entry = self.emit_param_inits(&function.params, &param_ir_names, entry)?;

        let body_entry = self.emit_arguments_init(body_entry)?;

        let mut inner_flow = StmtFlow::Open(body_entry);
        if let Some(body) = &function.body {
            for stmt in &body.stmts {
                if matches!(inner_flow, StmtFlow::Terminated) {
                    continue;
                }
                inner_flow = self.lower_stmt(stmt, inner_flow)?;
            }
        }

        if let StmtFlow::Open(b) = inner_flow {
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&fn_name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        for b in blocks {
            ir_function.push_block(b);
        }
        let function_id = self.module.push_function(ir_function);

        self.pop_function_context();

        let func_ref_const = self.module.add_constant(Constant::FunctionRef(function_id));
        let func_ref_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: func_ref_val,
                constant: func_ref_const,
            },
        );

        let callee_val = if let Some(home_object) = home_object {
            let env_val = self.create_method_env_with_home(block, &captured, home_object);
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            closure_val
        } else if captured.is_empty() {
            func_ref_val
        } else {
            let env_val = self.ensure_shared_env(block, &captured, key.span())?;
            let closure_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(closure_val),
                    builtin: Builtin::CreateClosure,
                    args: vec![func_ref_val, env_val],
                },
            );
            closure_val
        };

        Ok(callee_val)
    }

    /// 构建 getter/setter descriptor 对象 { get/set: fn, enumerable, configurable }
    pub(crate) fn build_descriptor(
        &mut self,
        accessor_kind: &str,
        fn_value: ValueId,
        enumerable: bool,
        configurable: bool,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let desc_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: desc_dest,
                capacity: 4,
            },
        );

        // descriptor[accessor_kind] = fn
        let key_const = self
            .module
            .add_constant(Constant::String(accessor_kind.to_string()));
        let key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: key_dest,
                constant: key_const,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: desc_dest,
                key: key_dest,
                value: fn_value,
            },
        );

        // descriptor.enumerable
        let enum_key = self
            .module
            .add_constant(Constant::String("enumerable".to_string()));
        let enum_key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: enum_key_dest,
                constant: enum_key,
            },
        );
        let enum_val_dest = self.alloc_value();
        let enum_const = self.module.add_constant(Constant::Bool(enumerable));
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: enum_val_dest,
                constant: enum_const,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: desc_dest,
                key: enum_key_dest,
                value: enum_val_dest,
            },
        );

        // descriptor.configurable
        let conf_key = self
            .module
            .add_constant(Constant::String("configurable".to_string()));
        let conf_key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: conf_key_dest,
                constant: conf_key,
            },
        );
        let conf_val_dest = self.alloc_value();
        let conf_const = self.module.add_constant(Constant::Bool(configurable));
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: conf_val_dest,
                constant: conf_const,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: desc_dest,
                key: conf_key_dest,
                value: conf_val_dest,
            },
        );

        Ok(desc_dest)
    }

    pub(crate) fn lower_array_expr(
        &mut self,
        arr: &swc_ast::ArrayLit,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let elem_count = arr.elems.len();
        // 根据元素数量分配容量（最少 4 个元素槽位减少扩容）
        let capacity = std::cmp::max(4, elem_count as u32);
        let arr_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewArray {
                dest: arr_dest,
                capacity,
            },
        );

        // 遍历元素：对每个元素 push 到数组
        for elem in &arr.elems {
            let val = match elem {
                Some(elem) => self.lower_expr(&elem.expr, block)?,
                None => {
                    // 稀疏数组的空位 → undefined
                    let undef_const = self.module.add_constant(Constant::Undefined);
                    let val_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: val_dest,
                            constant: undef_const,
                        },
                    );
                    val_dest
                }
            };
            // 使用 CallBuiltin(ArrayPush) 添加元素（同时自动更新 length）
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ArrayPush,
                    args: vec![arr_dest, val],
                },
            );
        }

        Ok(arr_dest)
    }

    pub(crate) fn lower_member_expr(
        &mut self,
        member: &swc_ast::MemberExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 拦截 Math 常量属性访问（Math.PI, Math.E 等）
        if let swc_ast::MemberProp::Ident(prop_ident) = &member.prop
            && let swc_ast::Expr::Ident(obj_ident) = member.obj.as_ref()
        {
            if obj_ident.sym == "Math" && self.scopes.lookup("Math").is_err() {
                let prop_name = prop_ident.sym.to_string();
                let is_math_const = matches!(
                    prop_name.as_str(),
                    "E" | "LN10" | "LN2" | "LOG10E" | "LOG2E" | "PI" | "SQRT1_2" | "SQRT2"
                );
                if is_math_const {
                    let math_const_name = format!("$0.Math.{}", prop_name);
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::LoadVar {
                            dest,
                            name: math_const_name,
                        },
                    );
                    return Ok(dest);
                }
            }

            // 拦截 Number 常量属性访问（Number.EPSILON, Number.MAX_VALUE 等）
            if obj_ident.sym == "Number" && self.scopes.lookup("Number").is_err() {
                let prop_name = prop_ident.sym.to_string();
                let is_number_const = matches!(
                    prop_name.as_str(),
                    "EPSILON"
                        | "MAX_VALUE"
                        | "MIN_VALUE"
                        | "MAX_SAFE_INTEGER"
                        | "MIN_SAFE_INTEGER"
                        | "NaN"
                        | "NEGATIVE_INFINITY"
                        | "POSITIVE_INFINITY"
                );
                if is_number_const {
                    let number_const_name = format!("$0.Number.{}", prop_name);
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::LoadVar {
                            dest,
                            name: number_const_name,
                        },
                    );
                    return Ok(dest);
                }
            }
        }

        let obj_val = self.lower_expr(&member.obj, block)?;

        let key = match &member.prop {
            swc_ast::MemberProp::Ident(ident) => {
                let key_const = self
                    .module
                    .add_constant(Constant::String(ident.sym.to_string()));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                key_dest
            }
            swc_ast::MemberProp::Computed(computed) => self.lower_expr(&computed.expr, block)?,
            swc_ast::MemberProp::PrivateName(name) => {
                let field_name = format!("#{}", name.name);
                let key_const = self.module.add_constant(Constant::String(field_name));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::PrivateGet,
                        args: vec![obj_val, key_dest],
                    },
                );
                return Ok(dest);
            }
        };

        let dest = self.alloc_value();
        match &member.prop {
            // Ident（命名属性）→ GetProp（走原型链，或读取 length 等内置属性）
            // Ident（命名属性）→ 检查是否为 Symbol 的静态属性（如 Symbol.dispose）
            swc_ast::MemberProp::Ident(ident) => {
                // 检查对象是否为 Symbol（编译时已知的 well-known symbol 访问）
                if let swc_ast::Expr::Ident(obj_ident) = member.obj.as_ref()
                    && obj_ident.sym == "Symbol"
                {
                    let prop_name = ident.sym.to_string();
                    // 将 Symbol.dispose 等映射为 well-known symbol
                    let wk_index = match prop_name.as_str() {
                        "iterator" => Some(WK_SYMBOL_ITERATOR),
                        "species" => Some(WK_SYMBOL_SPECIES),
                        "toStringTag" => Some(WK_SYMBOL_TO_STRING_TAG),
                        "asyncIterator" => Some(WK_SYMBOL_ASYNC_ITERATOR),
                        "hasInstance" => Some(WK_SYMBOL_HAS_INSTANCE),
                        "toPrimitive" => Some(WK_SYMBOL_TO_PRIMITIVE),
                        "dispose" => Some(WK_SYMBOL_DISPOSE),
                        "match" => Some(WK_SYMBOL_MATCH),
                        "asyncDispose" => Some(WK_SYMBOL_ASYNC_DISPOSE),
                        _ => None,
                    };
                    if let Some(idx) = wk_index {
                        let idx_const = self.module.add_constant(Constant::Number(idx as f64));
                        let idx_val = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: idx_val,
                                constant: idx_const,
                            },
                        );
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::SymbolWellKnown,
                                args: vec![idx_val],
                            },
                        );
                        return Ok(dest);
                    }
                }
                // 默认走 GetProp 路径
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest,
                        object: obj_val,
                        key,
                    },
                );
            }
            // Computed（计算属性）：数字字面量用 GetElem，其他用 GetProp
            swc_ast::MemberProp::Computed(_) => {
                // 检查 computed key 是否为数字字面量 → GetElem
                let use_get_elem = matches!(
                    member.prop,
                    swc_ast::MemberProp::Computed(swc_ast::ComputedPropName { ref expr, .. })
                        if matches!(expr.as_ref(), swc_ast::Expr::Lit(swc_ast::Lit::Num(_)))
                );
                if use_get_elem {
                    self.current_function.append_instruction(
                        block,
                        Instruction::GetElem {
                            dest,
                            object: obj_val,
                            index: key,
                        },
                    );
                } else {
                    self.current_function.append_instruction(
                        block,
                        Instruction::GetProp {
                            dest,
                            object: obj_val,
                            key,
                        },
                    );
                }
            }
            _ => unreachable!(),
        }
        Ok(dest)
    }

    /// 加载当前函数的闭包环境对象（$env 参数）
    pub(crate) fn load_env_object(&mut self, block: BasicBlockId) -> ValueId {
        let dest = self.alloc_value();
        let name = if let Some(ref env_name) = self.async_closure_env_ir_name {
            env_name.clone()
        } else {
            "$env".to_string()
        };
        self.current_function
            .append_instruction(block, Instruction::LoadVar { dest, name });
        dest
    }
}
