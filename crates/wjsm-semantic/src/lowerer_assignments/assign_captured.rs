use super::*;

impl Lowerer {
    pub(crate) fn lower_assign_eval_env(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        name: &str,
    ) -> Result<ValueId, LoweringError> {
        if assign.op == swc_ast::AssignOp::Assign {
            let mut current_block = block;
            let rhs = self.lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
            let block = self.append_eval_env_write(name, rhs, current_block)?;
            self.expr_merge_block = Some(block);
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
        let loaded = self.lower_eval_env_read(name, block)?;
        let mut current_block = self.resolve_store_block(block);
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
        let block = self.append_eval_env_write(name, dest, current_block)?;
        self.expr_merge_block = Some(block);
        Ok(dest)
    }

    /// 对捕获变量的赋值：通过 env 对象的 GetProp/SetProp 实现
    pub(crate) fn lower_assign_captured(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> Result<ValueId, LoweringError> {
        let mut current_block = block;
        let env_val = if self.binding_belongs_to_current_function(binding) {
            let env_val =
                self.ensure_shared_env(current_block, std::slice::from_ref(binding), assign.span)?;
            current_block = self.resolve_store_block(current_block);
            env_val
        } else {
            self.record_capture(binding.clone());
            self.load_env_object(current_block)
        };
        let key_val = self.append_env_key_const(current_block, binding);

        match assign.op {
            swc_ast::AssignOp::Assign => {
                let rhs = self.lower_expr(assign.right.as_ref(), current_block)?;
                let store_block = self.resolve_store_block(current_block);
                self.current_function.append_instruction(
                    store_block,
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
                    return self.lower_logical_assign_captured(
                        assign,
                        current_block,
                        binding,
                        env_val,
                        key_val,
                    );
                }
                // 算术/位运算复合赋值
                let bin_op = assign_op_to_binary(op).ok_or_else(|| {
                    self.error(assign.span, "unsupported compound assignment operator")
                })?;
                // 从 env 对象读取当前值
                let loaded = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::GetProp {
                        dest: loaded,
                        object: env_val,
                        key: key_val,
                    },
                );
                let rhs = self.lower_expr(assign.right.as_ref(), current_block)?;
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
                // 写回 env 对象
                let key_val2 = self.append_env_key_const(current_block, binding);
                self.current_function.append_instruction(
                    current_block,
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
        // 必须设置 expr_merge_block，否则上层 lower_expr_stmt/return 调用 resolve_store_block(原始 block) 时，
        // 因 captured 路径在 ensure_shared_env/resolve_store_block 后才在 continuation block 上挂 Branch + merge，
        // 导致 heuristic 可能看不到 Phi merge 或选中 stale pre-branch block，后续 return hits / 赋值结果 会用错 block → undefined。
        // 其他 short-circuit 路径（lower_logical 等）均在 Phi 后设置，此处对称补齐。
        self.expr_merge_block = Some(merge);
        Ok(result)
    }

    pub(crate) fn lower_logical_assign_eval_env(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        name: &str,
    ) -> Result<ValueId, LoweringError> {
        let env_val = self.load_eval_scope_env(block);
        let loaded = self.alloc_value();
        if self.eval_scope_record {
            let name_const = self.module.add_constant(Constant::String(name.to_string()));
            let name_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: name_val,
                    constant: name_const,
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(loaded),
                    builtin: Builtin::EvalGetBinding,
                    args: vec![env_val, name_val],
                },
            );
        } else {
            // LEGACY: flat-object eval scope bridge. ScopeRecord eval uses EvalGetBinding above;
            // keep this branch for non-ScopeRecord fallback paths until those are retired.
            let key_val = self.append_eval_env_key_const(block, name);
            self.current_function.append_instruction(
                block,
                Instruction::GetProp {
                    dest: loaded,
                    object: env_val,
                    key: key_val,
                },
            );
        }

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
        let assign_end = self.append_eval_env_write(name, rhs, assign_block)?;
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
        self.expr_merge_block = Some(merge);
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
