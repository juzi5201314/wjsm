use super::*;

impl Lowerer {
    pub(crate) fn lower_cond(
        &mut self,
        cond: &swc_ast::CondExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 评估条件表达式
        let test = self.lower_expr(&cond.test, block)?;
        let branch_block = self.resolve_store_block(block);
        // 若 resolve_store_block 返回的 block 含 Phi（来自嵌套逻辑/条件表达式），
        // 不能直接在其上设置 Branch，否则同一 block 有 Phi + Branch → WASM codegen 错误。
        let branch_block = if self.current_function.block(branch_block).is_some_and(|b| {
            b.instructions()
                .iter()
                .any(|i| matches!(i, Instruction::Phi { .. }))
        }) {
            let new_branch = self.current_function.new_block();
            self.current_function
                .set_terminator(branch_block, Terminator::Jump { target: new_branch });
            new_branch
        } else {
            branch_block
        };

        let cons_block = self.current_function.new_block();
        let alt_block = self.current_function.new_block();
        let merge = self.current_function.new_block();
        self.current_function.set_terminator(
            branch_block,
            Terminator::Branch {
                condition: test,
                true_block: cons_block,
                false_block: alt_block,
            },
        );

        let cons_val = self.lower_expr(&cond.cons, cons_block)?;
        let cons_end = self.resolve_store_block(cons_block);
        self.current_function
            .set_terminator(cons_end, Terminator::Jump { target: merge });

        let alt_val = self.lower_expr(&cond.alt, alt_block)?;
        let alt_end = self.resolve_store_block(alt_block);
        self.current_function
            .set_terminator(alt_end, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: cons_end,
                        value: cons_val,
                    },
                    PhiSource {
                        predecessor: alt_end,
                        value: alt_val,
                    },
                ],
            },
        );

        self.expr_merge_block = Some(merge);

        Ok(result)
    }
    // ── Comma expression ────────────────────────────────────────────────────

    pub(crate) fn lower_seq(
        &mut self,
        seq: &swc_ast::SeqExpr,
        mut block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let mut last = self.alloc_value();
        for expr in &seq.exprs {
            last = self.lower_expr_then_continue(expr, &mut block)?;
        }
        self.expr_merge_block = Some(block);
        Ok(last)
    }

    // ── Literals ────────────────────────────────────────────────────────────

    pub(crate) fn lower_literal(
        &mut self,
        lit: &swc_ast::Lit,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let constant = match lit {
            swc_ast::Lit::Num(num) => Constant::Number(num.value),
            swc_ast::Lit::Str(string) => {
                Constant::String(string.value.to_string_lossy().into_owned())
            }
            swc_ast::Lit::Bool(b) => Constant::Bool(b.value),
            swc_ast::Lit::BigInt(b) => Constant::BigInt(b.value.to_str_radix(10)),
            swc_ast::Lit::Regex(regex) => Constant::RegExp {
                pattern: regex.exp.to_string(),
                flags: regex.flags.to_string(),
            },
            swc_ast::Lit::Null(_) => Constant::Null,
            _ => {
                return Err(self.error(
                    lit.span(),
                    format!("unsupported literal kind `{}`", literal_kind(lit)),
                ));
            }
        };

        let constant = self.module.add_constant(constant);
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        Ok(dest)
    }

    // ── Helper: load bool constant ──────────────────────────────────────────

    pub(crate) fn load_bool_constant(&mut self, val: bool, block: BasicBlockId) -> ValueId {
        let constant = self.module.add_constant(Constant::Bool(val));
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        dest
    }

    // ── Flow helper ─────────────────────────────────────────────────────────
}
