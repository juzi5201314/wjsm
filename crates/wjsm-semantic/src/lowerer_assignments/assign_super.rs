use super::*;

/// super 属性赋值的 Reflect.get / Reflect.set 操作数（target、key、receiver）。
pub(crate) struct SuperPropAccess {
    base: ValueId,
    key: ValueId,
    this: ValueId,
}

impl Lowerer {
    pub(crate) fn lower_super_prop_access(
        &mut self,
        super_prop: &swc_ast::SuperPropExpr,
        block: &mut BasicBlockId,
    ) -> Result<SuperPropAccess, LoweringError> {
        if !self.eval_scope_record && !self.super_allowed {
            return Err(self.error(super_prop.span, "super is only valid inside methods"));
        }

        // SuperProperty 求值步骤 2：GetThisBinding 先于基座解析，派生构造器
        // this 处于 TDZ 时抛 ReferenceError（检查可能分叉，块就地推进）。
        let this_val = self.lower_this_checked(block)?;

        let base_val = self.alloc_value();
        if self.eval_scope_record {
            let env = self.load_eval_scope_env(*block);
            self.current_function.append_instruction(
                *block,
                Instruction::CallBuiltin {
                    dest: Some(base_val),
                    builtin: Builtin::EvalSuperBase,
                    args: vec![env],
                },
            );
        } else {
            self.current_function
                .append_instruction(*block, Instruction::GetSuperBase { dest: base_val });
        }

        let key = match &super_prop.prop {
            swc_ast::SuperProp::Ident(ident_name) => {
                let key_const = self
                    .module
                    .add_constant(Constant::String(ident_name.sym.to_string()));
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
            swc_ast::SuperProp::Computed(computed) => {
                self.lower_expr_then_continue(&computed.expr, block)?
            }
        };

        Ok(SuperPropAccess {
            base: base_val,
            key,
            this: this_val,
        })
    }

    fn emit_super_prop_get(&mut self, block: BasicBlockId, access: &SuperPropAccess) -> ValueId {
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::ReflectGet,
                args: vec![access.base, access.key, access.this],
            },
        );
        dest
    }

    pub(crate) fn emit_super_prop_set(
        &mut self,
        block: BasicBlockId,
        access: &SuperPropAccess,
        value: ValueId,
    ) {
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ReflectSet,
                args: vec![access.base, access.key, value, access.this],
            },
        );
    }

    pub(crate) fn lower_assign_super_prop(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        super_prop: &swc_ast::SuperPropExpr,
    ) -> Result<ValueId, LoweringError> {
        let mut current_block = block;
        let access = self.lower_super_prop_access(super_prop, &mut current_block)?;

        if assign.op == swc_ast::AssignOp::Assign {
            let rhs = self.lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
            self.emit_super_prop_set(current_block, &access, rhs);
            self.expr_merge_block = Some(current_block);
            return Ok(rhs);
        }

        if matches!(
            assign.op,
            swc_ast::AssignOp::AndAssign
                | swc_ast::AssignOp::OrAssign
                | swc_ast::AssignOp::NullishAssign
        ) {
            return self.lower_logical_assign_super(assign, current_block, access);
        }

        let bin_op = assign_op_to_binary(assign.op)
            .ok_or_else(|| self.error(assign.span, "unsupported compound assignment operator"))?;

        let loaded = self.emit_super_prop_get(current_block, &access);
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
        self.emit_super_prop_set(current_block, &access, dest);
        self.expr_merge_block = Some(current_block);
        Ok(dest)
    }

    fn lower_logical_assign_super(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        access: SuperPropAccess,
    ) -> Result<ValueId, LoweringError> {
        let loaded = self.emit_super_prop_get(block, &access);

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

        let rhs = self.lower_expr(assign.right.as_ref(), assign_block)?;
        let assign_end = self.resolve_store_block(assign_block);
        self.emit_super_prop_set(assign_end, &access, rhs);
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

    pub(crate) fn lower_update_super_prop(
        &mut self,
        update: &swc_ast::UpdateExpr,
        super_prop: &swc_ast::SuperPropExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let mut access_block = block;
        let access = self.lower_super_prop_access(super_prop, &mut access_block)?;
        let old_val = self.emit_super_prop_get(access_block, &access);
        // super getter 可抛出，必须在 ToNumeric 前中止并传播。
        let read_block = self.lower_value_exception_branch(access_block, old_val)?;

        let (num_val, new_val, math_block) =
            self.append_update_math(read_block, old_val, update.op)?;
        self.emit_super_prop_set(math_block, &access, new_val);
        // 分叉推进了块时必须上报延续块，否则后续语句会误写已终结的入口块。
        if math_block != block {
            self.expr_merge_block = Some(math_block);
        }

        Ok(if update.prefix { new_val } else { num_val })
    }
}
