use super::*;

impl Lowerer {
    pub(crate) fn lower_jsx_element(
        &mut self,
        jsx_el: &swc_ast::JSXElement,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 降低 tag 名
        let tag_val = self.lower_jsx_element_name(&jsx_el.opening.name, block)?;

        // 降低 props：spread 属性的异常分叉会推进 block，
        // 后续 children 与 CallBuiltin 必须落在推进后的块上。
        let mut current = block;
        let props_val = self.lower_jsx_attrs(&jsx_el.opening.attrs, &mut current)?;

        // 降低 children（作为数组）；嵌套子元素同样可能分叉推进 block
        let children_val = self.lower_jsx_children(&jsx_el.children, &mut current)?;

        // 调用 jsx_create_element(tag, props, children)
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            current,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::JsxCreateElement,
                args: vec![tag_val, props_val, children_val],
            },
        );
        if current != block {
            self.expr_merge_block = Some(current);
        }
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

        // 收集 children；嵌套子元素的异常分叉可能推进 block
        let mut current = block;
        let children_val = self.lower_jsx_children(&jsx_frag.children, &mut current)?;

        // 调用 jsx_create_element(tag, null, children)
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            current,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::JsxCreateElement,
                args: vec![tag_val, props_val, children_val],
            },
        );
        if current != block {
            self.expr_merge_block = Some(current);
        }
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

    /// 属性值/spread 源求值与异常分叉可能推进 block，调用方须以推进后的
    /// `*block` 作为后续发射点。
    pub(crate) fn lower_jsx_attrs(
        &mut self,
        attrs: &[swc_ast::JSXAttrOrSpread],
        block: &mut BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if attrs.is_empty() {
            // 无属性 → null
            let null_const = self.module.add_constant(Constant::Null);
            let null_val = self.alloc_value();
            self.current_function.append_instruction(
                *block,
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
            *block,
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

                    let attr_value = if let Some(value) = &attr.value {
                        match value {
                            swc_ast::JSXAttrValue::Str(s) => {
                                let str_val = s.value.to_string_lossy().into_owned();
                                let const_id = self.module.add_constant(Constant::String(str_val));
                                let val = self.alloc_value();
                                self.current_function.append_instruction(
                                    *block,
                                    Instruction::Const {
                                        dest: val,
                                        constant: const_id,
                                    },
                                );
                                val
                            }
                            swc_ast::JSXAttrValue::JSXExprContainer(expr_container) => {
                                match &expr_container.expr {
                                    swc_ast::JSXExpr::Expr(expr) => {
                                        self.lower_expr_then_continue(expr, block)?
                                    }
                                    swc_ast::JSXExpr::JSXEmptyExpr(_) => {
                                        // 空表达式 → true
                                        let true_const =
                                            self.module.add_constant(Constant::Bool(true));
                                        let val = self.alloc_value();
                                        self.current_function.append_instruction(
                                            *block,
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
                                let val = self.lower_jsx_element(el, *block)?;
                                *block = self.resolve_store_block(*block);
                                val
                            }
                            swc_ast::JSXAttrValue::JSXFragment(frag) => {
                                let val = self.lower_jsx_fragment(frag, *block)?;
                                *block = self.resolve_store_block(*block);
                                val
                            }
                        }
                    } else {
                        // 无值属性（如 <input disabled />）→ true
                        let true_const = self.module.add_constant(Constant::Bool(true));
                        let val = self.alloc_value();
                        self.current_function.append_instruction(
                            *block,
                            Instruction::Const {
                                dest: val,
                                constant: true_const,
                            },
                        );
                        val
                    };

                    // CreateDataProperty(obj, attr_name, attr_value)
                    let key_const = self.module.add_constant(Constant::String(attr_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        *block,
                        Instruction::Const {
                            dest: key_dest,
                            constant: key_const,
                        },
                    );
                    self.emit_create_data_property(*block, obj_dest, key_dest, attr_value);
                }
                swc_ast::JSXAttrOrSpread::SpreadElement(spread) => {
                    let source = self.lower_expr_then_continue(&spread.expr, block)?;
                    // 与对象字面量 spread 一致：源求值与 CopyDataProperties
                    // 的异常都必须传播，不得静默产生残缺 props。
                    if self.expr_exception_fork_allowed() && self.expr_can_throw(&spread.expr) {
                        *block = self.lower_value_exception_branch(*block, source)?;
                    }
                    *block = self.emit_object_spread_checked(*block, obj_dest, source)?;
                }
            }
        }

        Ok(obj_dest)
    }

    pub(crate) fn lower_jsx_children(
        &mut self,
        children: &[swc_ast::JSXElementChild],
        block: &mut BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if children.is_empty() {
            // 无 children → null
            let null_const = self.module.add_constant(Constant::Null);
            let null_val = self.alloc_value();
            self.current_function.append_instruction(
                *block,
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
            *block,
            Instruction::NewArray {
                dest: arr,
                capacity: children.len() as u32,
            },
        );

        // 子表达式/嵌套元素求值可能分叉推进 block，push 必须落在推进后的块上
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
                        *block,
                        Instruction::Const {
                            dest: val,
                            constant: str_const,
                        },
                    );
                    val
                }
                swc_ast::JSXElementChild::JSXExprContainer(expr_container) => {
                    match &expr_container.expr {
                        swc_ast::JSXExpr::Expr(expr) => {
                            self.lower_expr_then_continue(expr, block)?
                        }
                        swc_ast::JSXExpr::JSXEmptyExpr(_) => continue,
                    }
                }
                swc_ast::JSXElementChild::JSXElement(el) => {
                    let val = self.lower_jsx_element(el, *block)?;
                    *block = self.resolve_store_block(*block);
                    val
                }
                swc_ast::JSXElementChild::JSXFragment(frag) => {
                    let val = self.lower_jsx_fragment(frag, *block)?;
                    *block = self.resolve_store_block(*block);
                    val
                }
                _ => continue,
            };
            self.current_function.append_instruction(
                *block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ArrayPush,
                    args: vec![arr, child_val],
                },
            );
        }

        Ok(arr)
    }
}
