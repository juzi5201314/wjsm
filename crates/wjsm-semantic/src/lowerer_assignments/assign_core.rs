use super::*;

impl Lowerer {
    pub(crate) fn append_eval_var_leak_if_needed(
        &mut self,
        name: &str,
        kind: VarKind,
        value: ValueId,
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        if self.eval_var_writes_to_scope && matches!(kind, VarKind::Var) {
            return self.append_eval_env_write(name, value, block);
        }
        Ok(block)
    }

    pub(crate) fn lower_ident(
        &mut self,
        ident: &swc_ast::Ident,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let name = ident.sym.to_string();
        let module_id = self.current_module_id;

        // 命名空间局部（`import * as ns`）按 (导入方模块, local) 查找，避免跨模块同名覆盖（#44）。
        if let Some(mid) = module_id
            && let Some(ns_obj) = self
                .static_namespace_import_objects
                .get(&(mid, name.clone()))
                .copied()
        {
            return Ok(ns_obj);
        }

        // 命名导入别名按 (导入方模块, local) 查找。读取时复用 lower_ident 对捕获/共享 env
        // 的同一套判定：仅当绑定逃逸出当前函数或已进入共享 env 时才走 env 取值路径，
        // 否则直接 LoadVar。这样既保证被改写导出对导入方可见（live binding，#45），
        // 又不会在共享 env 从未创建时误读未初始化槽。
        if let Some(mid) = module_id
            && let Some(alias_ir_name) = self.import_aliases.get(&(mid, name.clone())).cloned()
        {
            let binding = crate::lowerer_modules::parse_ir_name_to_binding(&alias_ir_name);
            if !self.binding_belongs_to_current_function(&binding)
                || self.is_shared_binding(&binding)
            {
                return self.load_captured_binding(block, &binding);
            }
            let dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::LoadVar {
                    dest,
                    name: alias_ir_name,
                },
            );
            return Ok(dest);
        }

        if name == "eval" && self.scopes.lookup("eval").is_err() {
            let constant = self.module.add_constant(Constant::NativeCallableEval);
            let dest = self.alloc_value();
            self.current_function
                .append_instruction(block, Instruction::Const { dest, constant });
            return Ok(dest);
        }

        let (scope_id, _kind) = match self.scopes.lookup(&name) {
            Ok(found) => found,
            Err(msg) if msg.starts_with("undeclared identifier") && is_builtin_global(&name) => {
                // 变量查找失败 → 从全局对象按名读取属性
                // 全局对象已在模块初始化阶段通过 CreateGlobalObject 创建并存入 $0.$global
                let global_obj = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: global_obj,
                        name: "$0.$global".to_string(),
                    },
                );
                let key_const = self.module.add_constant(Constant::String(name));
                let key_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_val,
                        constant: key_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest,
                        object: global_obj,
                        key: key_val,
                    },
                );
                return Ok(dest);
            }
            Err(msg)
                if self.eval_scope_bridge_active() && msg.starts_with("undeclared identifier") =>
            {
                return self.lower_eval_env_read(&name, block);
            }
            Err(msg) => return Err(self.error(ident.span, msg)),
        };

        let binding = CapturedBinding::new(name.clone(), scope_id);
        if !self.binding_belongs_to_current_function(&binding) || self.is_shared_binding(&binding) {
            return self.load_captured_binding(block, &binding);
        }

        // 局部变量：直接 LoadVar
        let ir_name = format!("${scope_id}.{name}");
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest,
                name: ir_name,
            },
        );
        Ok(dest)
    }

    // ── Assignments ─────────────────────────────────────────────────────────

    pub(crate) fn lower_assign(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if let swc_ast::AssignTarget::Simple(simple) = &assign.left
            && let swc_ast::SimpleAssignTarget::SuperProp(super_prop) = simple
        {
            return self.lower_assign_super_prop(assign, block, super_prop);
        }

        // Handle member expression assignment targets (e.g. obj.prop = value).
        if let swc_ast::AssignTarget::Simple(simple) = &assign.left
            && let swc_ast::SimpleAssignTarget::Member(member_expr) = simple
        {
            let mut current_block = block;
            let obj_val = self.lower_expr_then_continue(&member_expr.obj, &mut current_block)?;
            let key = match &member_expr.prop {
                swc_ast::MemberProp::Ident(ident) => {
                    let name = ident.sym.to_string();
                    // __proto__ 赋值是 Object.setPrototypeOf 的语法糖（spec:
                    // __proto__ 是 Object.prototype 上的 accessor，setter 调用
                    // setPrototypeOf）。直接发射 CallBuiltin(ObjectSetPrototypeOf)
                    // 而非 SetProp，确保原型真正被设置（含循环检测、可扩展性检查）。
                    if name == "__proto__" && assign.op == swc_ast::AssignOp::Assign {
                        let value_val = self
                            .lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            current_block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::ObjectSetPrototypeOf,
                                args: vec![obj_val, value_val],
                            },
                        );
                        let continue_block =
                            self.lower_value_exception_branch(current_block, dest)?;
                        self.expr_merge_block = Some(continue_block);
                        return Ok(value_val);
                    }
                    let key_const = self.module.add_constant(Constant::String(name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        current_block,
                        Instruction::Const {
                            dest: key_dest,
                            constant: key_const,
                        },
                    );
                    key_dest
                }
                swc_ast::MemberProp::Computed(computed) => {
                    self.lower_expr_then_continue(&computed.expr, &mut current_block)?
                }
                swc_ast::MemberProp::PrivateName(name) => {
                    let field_name = format!("#{}", name.name);
                    let key_const = self.module.add_constant(Constant::String(field_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        current_block,
                        Instruction::Const {
                            dest: key_dest,
                            constant: key_const,
                        },
                    );
                    if assign.op == swc_ast::AssignOp::Assign {
                        let value_val = self
                            .lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            current_block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::PrivateSet,
                                args: vec![obj_val, key_dest, value_val],
                            },
                        );
                        self.expr_merge_block = Some(current_block);
                        return Ok(value_val);
                    }
                    let old_val = self.alloc_value();
                    self.current_function.append_instruction(
                        current_block,
                        Instruction::CallBuiltin {
                            dest: Some(old_val),
                            builtin: Builtin::PrivateGet,
                            args: vec![obj_val, key_dest],
                        },
                    );
                    let rhs_val =
                        self.lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
                    let bin_op = assign_op_to_binary(assign.op).ok_or_else(|| {
                        self.error(assign.span, "unsupported compound assignment operator")
                    })?;
                    let result = self.alloc_value();
                    self.current_function.append_instruction(
                        current_block,
                        Instruction::Binary {
                            dest: result,
                            op: bin_op,
                            lhs: old_val,
                            rhs: rhs_val,
                        },
                    );
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        current_block,
                        Instruction::CallBuiltin {
                            dest: Some(dest),
                            builtin: Builtin::PrivateSet,
                            args: vec![obj_val, key_dest, result],
                        },
                    );
                    self.expr_merge_block = Some(current_block);
                    return Ok(result);
                }
            };

            let is_computed = matches!(&member_expr.prop, swc_ast::MemberProp::Computed(_));
            if assign.op == swc_ast::AssignOp::Assign {
                // 简单赋值: obj.x = value 或 arr[computed] = value
                let value_val =
                    self.lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
                match &member_expr.prop {
                    swc_ast::MemberProp::Computed(_) => {
                        self.current_function.append_instruction(
                            current_block,
                            Instruction::SetElem {
                                object: obj_val,
                                index: key,
                                value: value_val,
                            },
                        );
                    }
                    _ => {
                        self.current_function.append_instruction(
                            current_block,
                            Instruction::SetProp {
                                object: obj_val,
                                key,
                                value: value_val,
                            },
                        );
                    }
                }
                self.expr_merge_block = Some(current_block);
                return Ok(value_val);
            }

            // 逻辑复合赋值需要短路求值，走专用路径
            if matches!(
                assign.op,
                swc_ast::AssignOp::AndAssign
                    | swc_ast::AssignOp::OrAssign
                    | swc_ast::AssignOp::NullishAssign
            ) {
                return self.lower_logical_assign_member(assign, current_block, obj_val, key);
            }

            // 算术/位运算复合赋值
            let bin_op = assign_op_to_binary(assign.op).ok_or_else(|| {
                self.error(assign.span, "unsupported compound assignment operator")
            })?;

            // 用 GetElem/GetProp 读取当前值（取决于是否为 computed 成员）
            let loaded = self.alloc_value();
            self.current_function.append_instruction(
                current_block,
                if is_computed {
                    Instruction::GetElem {
                        dest: loaded,
                        object: obj_val,
                        index: key,
                    }
                } else {
                    Instruction::GetProp {
                        dest: loaded,
                        object: obj_val,
                        key,
                    }
                },
            );

            let rhs = self.lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
            let dest = self.alloc_value();
            self.current_function.append_instruction(
                current_block,
                Instruction::Binary {
                    dest,
                    op: bin_op,
                    lhs: loaded,
                    rhs,
                },
            );

            let instr = if is_computed {
                Instruction::SetElem {
                    object: obj_val,
                    index: key,
                    value: dest,
                }
            } else {
                Instruction::SetProp {
                    object: obj_val,
                    key,
                    value: dest,
                }
            };
            self.current_function
                .append_instruction(current_block, instr);
            self.expr_merge_block = Some(current_block);

            return Ok(dest);
        }

        let name = match &assign.left {
            swc_ast::AssignTarget::Simple(simple) => match simple {
                swc_ast::SimpleAssignTarget::Ident(binding_ident) => {
                    binding_ident.id.sym.to_string()
                }
                _ => {
                    return Err(self.error(
                        assign.left.span(),
                        "only simple identifier assignment targets are supported",
                    ));
                }
            },
            swc_ast::AssignTarget::Pat(pat) => {
                if assign.op != swc_ast::AssignOp::Assign {
                    return Err(self.error(
                        assign.span,
                        "compound assignment with destructuring is not supported",
                    ));
                }
                let value = self.lower_expr(assign.right.as_ref(), block)?;
                let ir_pat = swc_ast::Pat::from(pat.clone());
                self.lower_destructure_pattern(&ir_pat, value, block, VarKind::Let)?;
                return Ok(value);
            }
        };

        // 性能优化：使用 lookup_for_assign 一次遍历完成 const 检查 + TDZ 检查 + scope 解析，
        // 避免 check_mutable and lookup 各自遍历 scope chain 的冗余。
        let (scope_id, kind) = match self.scopes.lookup_for_assign(&name) {
            Ok(found) => found,
            Err(msg)
                if self.eval_scope_bridge_active() && msg.starts_with("undeclared identifier") =>
            {
                if assign.op == swc_ast::AssignOp::Assign && is_builtin_global(&name) {
                    // 对 builtin global 的赋值 → 写入 $0.$global 属性
                    let global_obj = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::LoadVar {
                            dest: global_obj,
                            name: "$0.$global".to_string(),
                        },
                    );
                    let key_const = self.module.add_constant(Constant::String(name));
                    let key_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: key_val,
                            constant: key_const,
                        },
                    );
                    let rhs = self.lower_expr(assign.right.as_ref(), block)?;
                    self.current_function.append_instruction(
                        block,
                        Instruction::SetProp {
                            object: global_obj,
                            key: key_val,
                            value: rhs,
                        },
                    );
                    return Ok(rhs);
                }
                if self.strict_mode && !self.eval_scope_bridge_active() {
                    // strict script/module: 对未声明变量赋值 → ReferenceError
                    let msg_const = self.module.add_constant(Constant::String(format!(
                        "assignment to undeclared variable '{name}'"
                    )));
                    let msg_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: msg_val,
                            constant: msg_const,
                        },
                    );
                    let error_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::CallBuiltin {
                            dest: Some(error_val),
                            builtin: Builtin::ReferenceErrorConstructor,
                            args: vec![msg_val],
                        },
                    );
                    // 创建 dummy 值（在 throw 终止块之前分配）
                    let dummy = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: dummy,
                            constant: self.module.add_constant(Constant::Undefined),
                        },
                    );
                    self.emit_throw_value(block, error_val)?;
                    // emit_throw_value 已终止块；返回的 dummy 不会被使用
                    return Ok(dummy);
                }
                return self.lower_assign_eval_env(assign, block, &name);
            }
            Err(msg) => return Err(self.error(assign.span, msg)),
        };

        let binding = CapturedBinding::new(name.clone(), scope_id);
        if !self.binding_belongs_to_current_function(&binding) || self.is_shared_binding(&binding) {
            return self.lower_assign_captured(assign, block, &binding);
        }

        let ir_name = format!("${scope_id}.{name}");

        match assign.op {
            swc_ast::AssignOp::Assign => {
                let rhs = self.lower_expr(assign.right.as_ref(), block)?;
                let store_block = self.resolve_store_block(block);
                self.current_function.append_instruction(
                    store_block,
                    Instruction::StoreVar {
                        name: ir_name,
                        value: rhs,
                    },
                );
                let after_write_block =
                    self.append_eval_var_leak_if_needed(&name, kind, rhs, store_block)?;
                if after_write_block != store_block {
                    self.expr_merge_block = Some(after_write_block);
                }
                // 更新 Array 绑定跟踪：arr = [...] / new Array(...) -> 标记；arr = 其他 -> 取消标记。
                if is_array_constructor_expr(assign.right.as_ref()) {
                    self.array_bindings.insert((scope_id, name.clone()));
                } else {
                    self.array_bindings.remove(&(scope_id, name.clone()));
                }
                // 更新 TypedArray 绑定跟踪：arr = new Int32Array -> 标记；arr = 其他 -> 取消标记
                if is_typedarray_constructor_expr(assign.right.as_ref()) {
                    self.typedarray_bindings.insert((scope_id, name.clone()));
                } else {
                    self.typedarray_bindings.remove(&(scope_id, name.clone()));
                }
                // 更新 SharedArrayBuffer 绑定跟踪（与 TypedArray 平行）
                if is_sharedarraybuffer_constructor_expr(assign.right.as_ref()) {
                    self.sab_bindings.insert((scope_id, name.clone()));
                } else {
                    self.sab_bindings.remove(&(scope_id, name.clone()));
                }
                // 更新 DataView 绑定跟踪（专用宿主导入调用约定）。
                if is_dataview_constructor_expr(assign.right.as_ref()) {
                    self.dataview_bindings.insert((scope_id, name.clone()));
                } else {
                    self.dataview_bindings.remove(&(scope_id, name.clone()));
                }
                Ok(rhs)
            }
            op => {
                // 逻辑复合赋值需要短路求值，走专用路径
                if matches!(
                    op,
                    swc_ast::AssignOp::AndAssign
                        | swc_ast::AssignOp::OrAssign
                        | swc_ast::AssignOp::NullishAssign
                ) {
                    return self.lower_logical_assign(assign, block, ir_name);
                }

                let bin_op = assign_op_to_binary(op).ok_or_else(|| {
                    self.error(assign.span, "unsupported compound assignment operator")
                })?;

                let loaded = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: loaded,
                        name: ir_name.clone(),
                    },
                );

                let rhs = self.lower_expr(assign.right.as_ref(), block)?;
                let dest = self.alloc_value();

                self.current_function.append_instruction(
                    block,
                    Instruction::Binary {
                        dest,
                        op: bin_op,
                        lhs: loaded,
                        rhs,
                    },
                );

                self.current_function.append_instruction(
                    block,
                    Instruction::StoreVar {
                        name: ir_name,
                        value: dest,
                    },
                );
                let after_write_block =
                    self.append_eval_var_leak_if_needed(&name, kind, dest, block)?;
                if after_write_block != block {
                    self.expr_merge_block = Some(after_write_block);
                }

                Ok(dest)
            }
        }
    }
}
