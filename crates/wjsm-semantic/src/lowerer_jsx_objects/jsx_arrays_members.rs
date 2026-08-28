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
        // 每个元素 lower 后可能切换 basic block（shared env 父链解析等），
        // push 必须落在 lower_expr 的继续块上，否则会写到已终结的 block，phi 未定义。
        let mut current = block;
        for elem in &arr.elems {
            let Some(elem) = elem else {
                self.current_function.append_instruction(
                    current,
                    Instruction::CallBuiltin {
                        dest: None,
                        builtin: Builtin::ArrayPushHole,
                        args: vec![arr_dest],
                    },
                );
                continue;
            };

            let val = self.lower_expr_then_continue(&elem.expr, &mut current)?;
            // ArrayAccumulation：元素求值抛异常必须传播——既不能把 TAG_EXCEPTION
            // 存入数组，也不能继续求值后续元素或让 spread 静默展开为空。
            if self.expr_can_throw(&elem.expr) {
                current = self.lower_value_exception_branch(current, val)?;
            }
            if elem.spread.is_some() {
                current = self.emit_array_push_spread_checked(current, arr_dest, val)?;
            } else {
                self.current_function.append_instruction(
                    current,
                    Instruction::CallBuiltin {
                        dest: None,
                        builtin: Builtin::ArrayPush,
                        args: vec![arr_dest, val],
                    },
                );
            }
        }

        self.expr_merge_block = Some(current);
        Ok(arr_dest)
    }

    /// 降低成员访问表达式。`is_optional` 为真表示这是可选链的短路点（`obj?.key`），
    /// 默认路径改发 `OptionalGetProp` / `OptionalGetElem`，由后端对 null/undefined
    /// 提前返回 undefined；提前返回的特殊形态（Symbol / Math / Number 常量、
    /// Map/Set 内建）接收者恒非 nullish，不受该标志影响。
    pub(crate) fn lower_member_expr(
        &mut self,
        member: &swc_ast::MemberExpr,
        block: BasicBlockId,
        is_optional: bool,
    ) -> Result<ValueId, LoweringError> {
        // Symbol.xxx → well-known symbol（须在 GetProp 之前，否则 key 会变成普通字符串；
        // `Symbol` 名被词法/模块绑定遮蔽时禁用，走通用属性读取）。
        if let swc_ast::MemberProp::Ident(prop_ident) = &member.prop
            && let swc_ast::Expr::Ident(obj_ident) = member.obj.as_ref()
            && obj_ident.sym == "Symbol"
            && !self.global_intrinsic_shadowed("Symbol")
            && let Some(idx) =
                crate::wk_symbol_map::well_known_symbol_property_index(&prop_ident.sym)
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
            if obj_ident.sym == "Math" && !self.global_intrinsic_shadowed("Math") {
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
            if obj_ident.sym == "Number" && !self.global_intrinsic_shadowed("Number") {
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
        // Map/Set 绑定 + `keys().next().value`（最旧键淘汰惯用法）→ 直连 first-key 内建，
        // 免迭代器对象创建、.next 属性读取与原生调用分派链。仅匹配无实参、
        // 中间结果不落变量的精确形态（迭代器对象不可观察，语义等价）。
        if let swc_ast::MemberProp::Ident(prop_ident) = &member.prop
            && prop_ident.sym == "value"
            && let swc_ast::Expr::Call(next_call) = member.obj.as_ref()
            && next_call.args.is_empty()
            && let swc_ast::Callee::Expr(next_callee) = &next_call.callee
            && let swc_ast::Expr::Member(next_member) = next_callee.as_ref()
            && let swc_ast::MemberProp::Ident(next_prop) = &next_member.prop
            && next_prop.sym == "next"
            && let swc_ast::Expr::Call(keys_call) = next_member.obj.as_ref()
            && keys_call.args.is_empty()
            && let swc_ast::Callee::Expr(keys_callee) = &keys_call.callee
            && let swc_ast::Expr::Member(keys_member) = keys_callee.as_ref()
            && let swc_ast::MemberProp::Ident(keys_prop) = &keys_member.prop
            && (keys_prop.sym == "keys" || keys_prop.sym == "values")
            && let swc_ast::Expr::Ident(receiver_ident) = keys_member.obj.as_ref()
            && (self.is_map_binding(receiver_ident) || self.is_set_binding(receiver_ident))
        {
            let obj_val = self.lower_expr_then_continue(&keys_member.obj, &mut current_block)?;
            let dest = self.alloc_value();
            self.current_function.append_instruction(
                current_block,
                Instruction::CallBuiltin {
                    dest: Some(dest),
                    builtin: Builtin::MapSetFirstKey,
                    args: vec![obj_val],
                },
            );
            self.expr_merge_block = Some(current_block);
            return Ok(dest);
        }
        let obj_val = self.lower_expr_then_continue(&member.obj, &mut current_block)?;
        // 命名空间对象（`import * as ns`）的导出属性已作为 live getter 预装在对象上，
        // 普通 GetProp 即可触发 getter 取最新值，无需快照填充（#45）。
        self.lower_member_expr_from_object(member, obj_val, &mut current_block, is_optional)
    }

    pub(crate) fn lower_member_expr_from_object(
        &mut self,
        member: &swc_ast::MemberExpr,
        obj_val: ValueId,
        block: &mut BasicBlockId,
        is_optional: bool,
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
                let field_name =
                    self.resolve_private_storage_name(name.name.as_ref(), name.span)?;
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
                // Map/Set 绑定 + `size` 访问器 → 直连 size 内建（免通用
                // Get + accessor getter 调用链）。
                if ident.sym == "size"
                    && let swc_ast::Expr::Ident(obj_ident) = member.obj.as_ref()
                    && (self.is_map_binding(obj_ident) || self.is_set_binding(obj_ident))
                {
                    self.current_function.append_instruction(
                        *block,
                        Instruction::CallBuiltin {
                            dest: Some(dest),
                            builtin: Builtin::MapSetGetSize,
                            args: vec![obj_val],
                        },
                    );
                    self.expr_merge_block = Some(*block);
                    return Ok(dest);
                }
                // 检查对象是否为 Symbol（编译时已知的 well-known symbol 访问；
                // `Symbol` 名被词法/模块绑定遮蔽时禁用）
                if let swc_ast::Expr::Ident(obj_ident) = member.obj.as_ref()
                    && obj_ident.sym == "Symbol"
                    && !self.global_intrinsic_shadowed("Symbol")
                {
                    let prop_name = ident.sym.to_string();
                    if let Some(idx) =
                        crate::wk_symbol_map::well_known_symbol_property_index(&prop_name)
                    {
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
                // 默认走 GetProp 路径；可选链短路点改发 OptionalGetProp。
                let instruction = if is_optional {
                    Instruction::OptionalGetProp {
                        dest,
                        object: obj_val,
                        key,
                    }
                } else {
                    Instruction::GetProp {
                        dest,
                        object: obj_val,
                        key,
                    }
                };
                self.current_function
                    .append_instruction(*block, instruction);
            }
            // Computed（计算属性）：统一走 GetElem。GetElem 在后端按 key 类型分派——
            // 数组 + 数字 key → 元素；否则 → 命名属性（obj_get，处理对象/数组 .length/原型/函数）。
            // 旧逻辑「仅数字字面量用 GetElem，其余用 GetProp」会让 a[变量] 漏掉数组元素路径。
            swc_ast::MemberProp::Computed(_) => {
                let instruction = if is_optional {
                    Instruction::OptionalGetElem {
                        dest,
                        object: obj_val,
                        key,
                    }
                } else {
                    Instruction::GetElem {
                        dest,
                        object: obj_val,
                        index: key,
                    }
                };
                self.current_function
                    .append_instruction(*block, instruction);
            }
            _ => unreachable!(),
        }
        self.expr_merge_block = Some(*block);
        Ok(dest)
    }

    /// 加载当前函数的闭包环境对象（$env 参数）
    pub(crate) fn load_env_object(&mut self, block: BasicBlockId) -> ValueId {
        let dest = self.alloc_value();
        let name = if let Some(env_name) = &self.async_closure_env_ir_name {
            env_name.clone()
        } else {
            "$env".to_string()
        };
        self.current_function
            .append_instruction(block, Instruction::LoadVar { dest, name });
        dest
    }
}
