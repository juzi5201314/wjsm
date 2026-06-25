use super::*;

impl Lowerer {
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

        // 遍历元素：普通元素 push；spread 元素按 iterator 协议展开。
        for elem in &arr.elems {
            let Some(elem) = elem else {
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: None,
                        builtin: Builtin::ArrayPushHole,
                        args: vec![arr_dest],
                    },
                );
                continue;
            };

            let val = self.lower_expr(&elem.expr, block)?;
            let builtin = if elem.spread.is_some() {
                Builtin::ArrayPushSpread
            } else {
                Builtin::ArrayPush
            };
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin,
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
        // Symbol.xxx → well-known symbol（须在 GetProp 之前，否则 key 会变成普通字符串）
        if let swc_ast::MemberProp::Ident(prop_ident) = &member.prop
            && let swc_ast::Expr::Ident(obj_ident) = member.obj.as_ref()
            && obj_ident.sym == "Symbol"
            && let Some(idx) = crate::wk_symbol_map::well_known_symbol_property_index(
                &prop_ident.sym,
            )
        {
            let idx_const = self.module.add_constant(Constant::Number(idx as f64));
            let idx_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: idx_val,
                    constant: idx_const,
                },
            );
            let dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(dest),
                    builtin: Builtin::SymbolWellKnown,
                    args: vec![idx_val],
                },
            );
            self.expr_merge_block = Some(block);
            return Ok(dest);
        }

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

        let mut current_block = block;
        let obj_val = self.lower_expr_then_continue(&member.obj, &mut current_block)?;
        if let swc_ast::MemberProp::Ident(prop_ident) = &member.prop
            && let swc_ast::Expr::Ident(obj_ident) = member.obj.as_ref()
            && let Some(&ns_obj) = self
                .static_namespace_import_objects
                .get(&obj_ident.sym.to_string())
            && ns_obj == obj_val
        {
            self.ensure_static_namespace_prop(ns_obj, &prop_ident.sym.to_string(), current_block);
        }

        self.lower_member_expr_from_object(member, obj_val, &mut current_block)
    }

    pub(crate) fn lower_member_expr_from_object(
        &mut self,
        member: &swc_ast::MemberExpr,
        obj_val: ValueId,
        block: &mut BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let key = match &member.prop {
            swc_ast::MemberProp::Ident(ident) => {
                let key_const = self
                    .module
                    .add_constant(Constant::String(ident.sym.to_string()));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    *block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                key_dest
            }
            swc_ast::MemberProp::Computed(computed) => {
                self.lower_expr_then_continue(&computed.expr, block)?
            }
            swc_ast::MemberProp::PrivateName(name) => {
                let field_name = format!("#{}", name.name);
                let key_const = self.module.add_constant(Constant::String(field_name));
                let key_dest = self.alloc_value();
                self.current_function.append_instruction(
                    *block,
                    Instruction::Const {
                        dest: key_dest,
                        constant: key_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    *block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::PrivateGet,
                        args: vec![obj_val, key_dest],
                    },
                );
                self.expr_merge_block = Some(*block);
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
                    if let Some(idx) = crate::wk_symbol_map::well_known_symbol_property_index(
                        &prop_name,
                    ) {
                        let idx_const = self.module.add_constant(Constant::Number(idx as f64));
                        let idx_val = self.alloc_value();
                        self.current_function.append_instruction(
                            *block,
                            Instruction::Const {
                                dest: idx_val,
                                constant: idx_const,
                            },
                        );
                        self.current_function.append_instruction(
                            *block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::SymbolWellKnown,
                                args: vec![idx_val],
                            },
                        );
                        self.expr_merge_block = Some(*block);
                        return Ok(dest);
                    }
                }
                // 默认走 GetProp 路径
                self.current_function.append_instruction(
                    *block,
                    Instruction::GetProp {
                        dest,
                        object: obj_val,
                        key,
                    },
                );
            }
            // Computed（计算属性）：统一走 GetElem。GetElem 在后端按 key 类型分派——
            // 数组 + 数字 key → 元素；否则 → 命名属性（obj_get，处理对象/数组 .length/原型/函数）。
            // 旧逻辑「仅数字字面量用 GetElem，其余用 GetProp」会让 a[变量] 漏掉数组元素路径。
            swc_ast::MemberProp::Computed(_) => {
                self.current_function.append_instruction(
                    *block,
                    Instruction::GetElem {
                        dest,
                        object: obj_val,
                        index: key,
                    },
                );
            }
            _ => unreachable!(),
        }
        self.expr_merge_block = Some(*block);
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
