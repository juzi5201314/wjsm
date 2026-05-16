use super::*;

impl Lowerer {
    pub(crate) fn append_eval_var_leak_if_needed(
        &mut self,
        name: &str,
        kind: VarKind,
        value: ValueId,
        block: BasicBlockId,
    ) {
        if self.eval_var_writes_to_scope && matches!(kind, VarKind::Var) {
            self.append_eval_env_write(name, value, block);
        }
    }

    pub(crate) fn lower_ident(
        &mut self,
        ident: &swc_ast::Ident,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let name = ident.sym.to_string();

        if let Some(alias_ir_name) = self.import_aliases.get(&name).cloned() {
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
            Err(msg)
                if self.eval_scope_bridge_active() && msg.starts_with("undeclared identifier") =>
            {
                return Ok(self.lower_eval_env_read(&name, block));
            }
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
        // Handle member expression assignment targets (e.g. obj.prop = value).
        if let swc_ast::AssignTarget::Simple(simple) = &assign.left {
            if let swc_ast::SimpleAssignTarget::Member(member_expr) = simple {
                let obj_val = self.lower_expr(&member_expr.obj, block)?;
                let key = match &member_expr.prop {
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
                    swc_ast::MemberProp::Computed(computed) => {
                        self.lower_expr(&computed.expr, block)?
                    }
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
                        if assign.op == swc_ast::AssignOp::Assign {
                            let value_val = self.lower_expr(assign.right.as_ref(), block)?;
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: Builtin::PrivateSet,
                                    args: vec![obj_val, key_dest, value_val],
                                },
                            );
                            return Ok(value_val);
                        }
                        let old_val = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(old_val),
                                builtin: Builtin::PrivateGet,
                                args: vec![obj_val, key_dest],
                            },
                        );
                        let rhs_val = self.lower_expr(assign.right.as_ref(), block)?;
                        let bin_op = assign_op_to_binary(assign.op).ok_or_else(|| {
                            self.error(assign.span, "unsupported compound assignment operator")
                        })?;
                        let result = self.alloc_value();
                        match bin_op {
                            BinaryOp::Mod => {
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::CallBuiltin {
                                        dest: Some(result),
                                        builtin: Builtin::F64Mod,
                                        args: vec![old_val, rhs_val],
                                    },
                                );
                            }
                            BinaryOp::Exp => {
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::CallBuiltin {
                                        dest: Some(result),
                                        builtin: Builtin::F64Exp,
                                        args: vec![old_val, rhs_val],
                                    },
                                );
                            }
                            _ => {
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::Binary {
                                        dest: result,
                                        op: bin_op,
                                        lhs: old_val,
                                        rhs: rhs_val,
                                    },
                                );
                            }
                        }
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::PrivateSet,
                                args: vec![obj_val, key_dest, result],
                            },
                        );
                        return Ok(result);
                    }
                };

                let is_computed = matches!(&member_expr.prop, swc_ast::MemberProp::Computed(_));
                if assign.op == swc_ast::AssignOp::Assign {
                    // 简单赋值: obj.x = value 或 arr[computed] = value
                    let value_val = self.lower_expr(assign.right.as_ref(), block)?;
                    match &member_expr.prop {
                        swc_ast::MemberProp::Computed(_) => {
                            self.current_function.append_instruction(
                                block,
                                Instruction::SetElem {
                                    object: obj_val,
                                    index: key,
                                    value: value_val,
                                },
                            );
                        }
                        _ => {
                            self.current_function.append_instruction(
                                block,
                                Instruction::SetProp {
                                    object: obj_val,
                                    key,
                                    value: value_val,
                                },
                            );
                        }
                    }
                    return Ok(value_val);
                }

                // 逻辑复合赋值需要短路求值，走专用路径
                if matches!(
                    assign.op,
                    swc_ast::AssignOp::AndAssign
                        | swc_ast::AssignOp::OrAssign
                        | swc_ast::AssignOp::NullishAssign
                ) {
                    return self.lower_logical_assign_member(assign, block, obj_val, key);
                }

                // 算术/位运算复合赋值
                let bin_op = assign_op_to_binary(assign.op).ok_or_else(|| {
                    self.error(assign.span, "unsupported compound assignment operator")
                })?;

                // 用 GetElem/GetProp 读取当前值（取决于是否为 computed 成员）
                let loaded = self.alloc_value();
                self.current_function.append_instruction(
                    block,
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

                let rhs = self.lower_expr(assign.right.as_ref(), block)?;
                let dest = self.alloc_value();

                // Mod 和 Exp 需要使用 CallBuiltin
                match bin_op {
                    BinaryOp::Mod => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::F64Mod,
                                args: vec![loaded, rhs],
                            },
                        );
                    }
                    BinaryOp::Exp => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::F64Exp,
                                args: vec![loaded, rhs],
                            },
                        );
                    }
                    _ => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::Binary {
                                dest,
                                op: bin_op,
                                lhs: loaded,
                                rhs,
                            },
                        );
                    }
                }
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
                self.current_function.append_instruction(block, instr);

                return Ok(dest);
            }
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
                self.current_function.append_instruction(
                    block,
                    Instruction::StoreVar {
                        name: ir_name,
                        value: rhs,
                    },
                );
                self.append_eval_var_leak_if_needed(&name, kind, rhs, block);
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

                // Mod 和 Exp 需要使用 CallBuiltin
                match bin_op {
                    BinaryOp::Mod => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::F64Mod,
                                args: vec![loaded, rhs],
                            },
                        );
                    }
                    BinaryOp::Exp => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::F64Exp,
                                args: vec![loaded, rhs],
                            },
                        );
                    }
                    _ => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::Binary {
                                dest,
                                op: bin_op,
                                lhs: loaded,
                                rhs,
                            },
                        );
                    }
                }

                self.current_function.append_instruction(
                    block,
                    Instruction::StoreVar {
                        name: ir_name,
                        value: dest,
                    },
                );
                self.append_eval_var_leak_if_needed(&name, kind, dest, block);

                Ok(dest)
            }
        }
    }

    pub(crate) fn lower_assign_eval_env(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        name: &str,
    ) -> Result<ValueId, LoweringError> {
        if assign.op == swc_ast::AssignOp::Assign {
            let rhs = self.lower_expr(assign.right.as_ref(), block)?;
            self.append_eval_env_write(name, rhs, block);
            return Ok(rhs);
        }

        if matches!(
            assign.op,
            swc_ast::AssignOp::AndAssign
                | swc_ast::AssignOp::OrAssign
                | swc_ast::AssignOp::NullishAssign
        ) {
            return self.lower_logical_assign_eval_env(assign, block, name);
        }

        let bin_op = assign_op_to_binary(assign.op)
            .ok_or_else(|| self.error(assign.span, "unsupported compound assignment operator"))?;
        let loaded = self.lower_eval_env_read(name, block);
        let rhs = self.lower_expr(assign.right.as_ref(), block)?;
        let dest = self.alloc_value();
        match bin_op {
            BinaryOp::Mod => {
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::F64Mod,
                        args: vec![loaded, rhs],
                    },
                );
            }
            BinaryOp::Exp => {
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::F64Exp,
                        args: vec![loaded, rhs],
                    },
                );
            }
            _ => {
                self.current_function.append_instruction(
                    block,
                    Instruction::Binary {
                        dest,
                        op: bin_op,
                        lhs: loaded,
                        rhs,
                    },
                );
            }
        }
        self.append_eval_env_write(name, dest, block);
        Ok(dest)
    }

    /// 对捕获变量的赋值：通过 env 对象的 GetProp/SetProp 实现
    pub(crate) fn lower_assign_captured(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> Result<ValueId, LoweringError> {
        let env_val = if self.binding_belongs_to_current_function(binding) {
            self.shared_env_value()
                .expect("shared binding must have a materialized env")
        } else {
            self.record_capture(binding.clone());
            self.load_env_object(block)
        };
        let key_val = self.append_env_key_const(block, binding);

        match assign.op {
            swc_ast::AssignOp::Assign => {
                let rhs = self.lower_expr(assign.right.as_ref(), block)?;
                self.current_function.append_instruction(
                    block,
                    Instruction::SetProp {
                        object: env_val,
                        key: key_val,
                        value: rhs,
                    },
                );
                Ok(rhs)
            }
            op => {
                // 逻辑复合赋值需短路求值 → 走专用路径
                if matches!(
                    op,
                    swc_ast::AssignOp::AndAssign
                        | swc_ast::AssignOp::OrAssign
                        | swc_ast::AssignOp::NullishAssign
                ) {
                    return self
                        .lower_logical_assign_captured(assign, block, binding, env_val, key_val);
                }
                // 算术/位运算复合赋值
                let bin_op = assign_op_to_binary(op).ok_or_else(|| {
                    self.error(assign.span, "unsupported compound assignment operator")
                })?;
                // 从 env 对象读取当前值
                let loaded = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest: loaded,
                        object: env_val,
                        key: key_val,
                    },
                );
                let rhs = self.lower_expr(assign.right.as_ref(), block)?;
                let dest = self.alloc_value();
                match bin_op {
                    BinaryOp::Mod => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::F64Mod,
                                args: vec![loaded, rhs],
                            },
                        );
                    }
                    BinaryOp::Exp => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::F64Exp,
                                args: vec![loaded, rhs],
                            },
                        );
                    }
                    _ => {
                        self.current_function.append_instruction(
                            block,
                            Instruction::Binary {
                                dest,
                                op: bin_op,
                                lhs: loaded,
                                rhs,
                            },
                        );
                    }
                }
                // 写回 env 对象
                let key_val2 = self.append_env_key_const(block, binding);
                self.current_function.append_instruction(
                    block,
                    Instruction::SetProp {
                        object: env_val,
                        key: key_val2,
                        value: dest,
                    },
                );
                Ok(dest)
            }
        }
    }

    /// 逻辑复合赋值到捕获变量（通过 env 对象）
    pub(crate) fn lower_logical_assign_captured(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        binding: &CapturedBinding,
        env_val: ValueId,
        key_val: ValueId,
    ) -> Result<ValueId, LoweringError> {
        // 从 env 读取当前值
        let loaded = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::GetProp {
                dest: loaded,
                object: env_val,
                key: key_val,
            },
        );

        let assign_block = self.current_function.new_block();
        let merge = self.current_function.new_block();

        let condition = if matches!(assign.op, swc_ast::AssignOp::NullishAssign) {
            let is_nullish = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Unary {
                    dest: is_nullish,
                    op: UnaryOp::IsNullish,
                    value: loaded,
                },
            );
            is_nullish
        } else {
            loaded
        };

        let (true_block, false_block) = match assign.op {
            swc_ast::AssignOp::AndAssign => (assign_block, merge),
            swc_ast::AssignOp::OrAssign => (merge, assign_block),
            swc_ast::AssignOp::NullishAssign => (assign_block, merge),
            _ => unreachable!(),
        };
        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition,
                true_block,
                false_block,
            },
        );

        let rhs = self.lower_expr(assign.right.as_ref(), assign_block)?;
        let assign_end = self.resolve_store_block(assign_block);
        let key_val2 = self.append_env_key_const(assign_end, binding);
        self.current_function.append_instruction(
            assign_end,
            Instruction::SetProp {
                object: env_val,
                key: key_val2,
                value: rhs,
            },
        );
        self.current_function
            .set_terminator(assign_end, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: block,
                        value: loaded,
                    },
                    PhiSource {
                        predecessor: assign_end,
                        value: rhs,
                    },
                ],
            },
        );
        Ok(result)
    }

    pub(crate) fn lower_logical_assign_eval_env(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        name: &str,
    ) -> Result<ValueId, LoweringError> {
        let env_val = self.load_eval_scope_env(block);
        let key_val = self.append_eval_env_key_const(block, name);
        let loaded = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::GetProp {
                dest: loaded,
                object: env_val,
                key: key_val,
            },
        );

        let assign_block = self.current_function.new_block();
        let merge = self.current_function.new_block();

        let condition = if matches!(assign.op, swc_ast::AssignOp::NullishAssign) {
            let is_nullish = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Unary {
                    dest: is_nullish,
                    op: UnaryOp::IsNullish,
                    value: loaded,
                },
            );
            is_nullish
        } else {
            loaded
        };

        let (true_block, false_block) = match assign.op {
            swc_ast::AssignOp::AndAssign => (assign_block, merge),
            swc_ast::AssignOp::OrAssign => (merge, assign_block),
            swc_ast::AssignOp::NullishAssign => (assign_block, merge),
            _ => unreachable!(),
        };
        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition,
                true_block,
                false_block,
            },
        );

        let rhs = self.lower_expr(assign.right.as_ref(), assign_block)?;
        let assign_end = self.resolve_store_block(assign_block);
        self.append_eval_env_write(name, rhs, assign_end);
        self.current_function
            .set_terminator(assign_end, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: block,
                        value: loaded,
                    },
                    PhiSource {
                        predecessor: assign_end,
                        value: rhs,
                    },
                ],
            },
        );
        Ok(result)
    }

    /// Lower logical compound assignment `&&=`, `||=`, `??=` with short-circuit CFG.
    /// Decomposed into LoadVar + Branch(Phi) + StoreVar just like lower_logical.
    pub(crate) fn lower_logical_assign(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        ir_name: String,
    ) -> Result<ValueId, LoweringError> {
        // 1. 加载当前值
        let loaded = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: loaded,
                name: ir_name.clone(),
            },
        );

        // 2. 创建 assign block 和 merge block
        let assign_block = self.current_function.new_block();
        let merge = self.current_function.new_block();

        // 3. 确定 condition 和分支目标
        let condition = if matches!(assign.op, swc_ast::AssignOp::NullishAssign) {
            let is_nullish = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Unary {
                    dest: is_nullish,
                    op: UnaryOp::IsNullish,
                    value: loaded,
                },
            );
            is_nullish
        } else {
            loaded
        };

        let (true_target, false_target) = match assign.op {
            swc_ast::AssignOp::AndAssign => (assign_block, merge),
            swc_ast::AssignOp::OrAssign => (merge, assign_block),
            swc_ast::AssignOp::NullishAssign => (assign_block, merge),
            _ => unreachable!(),
        };

        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition,
                true_block: true_target,
                false_block: false_target,
            },
        );

        // 4. 在 assign_block 中降低右值并写回
        let rhs = self.lower_expr(assign.right.as_ref(), assign_block)?;
        let assign_end = self.resolve_store_block(assign_block);
        self.current_function.append_instruction(
            assign_end,
            Instruction::StoreVar {
                name: ir_name,
                value: rhs,
            },
        );
        self.current_function
            .set_terminator(assign_end, Terminator::Jump { target: merge });

        // 5. 在 merge 处用 Phi 合并
        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: block,
                        value: loaded,
                    },
                    PhiSource {
                        predecessor: assign_end,
                        value: rhs,
                    },
                ],
            },
        );

        Ok(result)
    }

    /// Lower logical compound assignment to member expression target (&&=, ||=, ??=)
    /// with short-circuit CFG, using GetProp/SetProp instead of LoadVar/StoreVar.
    pub(crate) fn lower_logical_assign_member(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        obj_val: ValueId,
        key: ValueId,
    ) -> Result<ValueId, LoweringError> {
        // 1. 加载当前值 (GetProp)
        let loaded = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::GetProp {
                dest: loaded,
                object: obj_val,
                key,
            },
        );

        // 2. 创建 assign block 和 merge block
        let assign_block = self.current_function.new_block();
        let merge = self.current_function.new_block();

        // 3. 确定 condition 和分支目标
        let condition = if matches!(assign.op, swc_ast::AssignOp::NullishAssign) {
            let is_nullish = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Unary {
                    dest: is_nullish,
                    op: UnaryOp::IsNullish,
                    value: loaded,
                },
            );
            is_nullish
        } else {
            loaded
        };

        let (true_target, false_target) = match assign.op {
            swc_ast::AssignOp::AndAssign => (assign_block, merge),
            swc_ast::AssignOp::OrAssign => (merge, assign_block),
            swc_ast::AssignOp::NullishAssign => (assign_block, merge),
            _ => unreachable!(),
        };

        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition,
                true_block: true_target,
                false_block: false_target,
            },
        );

        // 4. 在 assign_block 中降低右值并写回 (SetProp)
        let rhs = self.lower_expr(assign.right.as_ref(), assign_block)?;
        let assign_end = self.resolve_store_block(assign_block);
        self.current_function.append_instruction(
            assign_end,
            Instruction::SetProp {
                object: obj_val,
                key,
                value: rhs,
            },
        );
        self.current_function
            .set_terminator(assign_end, Terminator::Jump { target: merge });

        // 5. 在 merge 处用 Phi 合并
        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: block,
                        value: loaded,
                    },
                    PhiSource {
                        predecessor: assign_end,
                        value: rhs,
                    },
                ],
            },
        );

        Ok(result)
    }

    // ── Binary operators ────────────────────────────────────────────────────
}
