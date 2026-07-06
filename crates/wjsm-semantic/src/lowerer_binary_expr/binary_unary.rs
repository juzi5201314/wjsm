use super::*;

impl Lowerer {
    /// 判断表达式在其自身求值时是否可能直接返回 TAG_EXCEPTION，从而需要异常检查分叉。
    /// 涵盖调用、成员读取、算术/位运算（含 BigInt 与 Number 混合时的 TypeError、`>>>` 与 BigInt）等。
    /// 子表达式的异常由各自经 `lower_expr_then_continue` 的求值负责传播。
    /// 刻意排除 Await/Yield（异步状态机自有续延处理）。
    /// Assign 包含：setter 调用（如 __proto__ 赋值）可能抛 TypeError。
    pub(crate) fn expr_can_throw(&self, expr: &swc_ast::Expr) -> bool {
        match expr {
            swc_ast::Expr::Assign(_) // setter / __proto__ 赋值可能抛 TypeError
            | swc_ast::Expr::Call(_)
            | swc_ast::Expr::New(_)
            | swc_ast::Expr::Member(_)
            | swc_ast::Expr::OptChain(_)
            | swc_ast::Expr::TaggedTpl(_) => true,
            swc_ast::Expr::Bin(bin) => match bin.op {
                swc_ast::BinaryOp::Add
                | swc_ast::BinaryOp::Sub
                | swc_ast::BinaryOp::Mul
                | swc_ast::BinaryOp::Div
                | swc_ast::BinaryOp::Mod
                | swc_ast::BinaryOp::Exp
                | swc_ast::BinaryOp::In
                | swc_ast::BinaryOp::InstanceOf
                | swc_ast::BinaryOp::BitOr
                | swc_ast::BinaryOp::BitXor
                | swc_ast::BinaryOp::BitAnd
                | swc_ast::BinaryOp::LShift
                | swc_ast::BinaryOp::RShift
                | swc_ast::BinaryOp::ZeroFillRShift => true,
                _ => {
                    self.expr_can_throw(bin.left.as_ref())
                        || self.expr_can_throw(bin.right.as_ref())
                }
            },
            swc_ast::Expr::Paren(p) => self.expr_can_throw(&p.expr),
            swc_ast::Expr::TsAs(e) => self.expr_can_throw(&e.expr),
            swc_ast::Expr::TsNonNull(e) => self.expr_can_throw(&e.expr),
            swc_ast::Expr::TsConstAssertion(e) => self.expr_can_throw(&e.expr),
            swc_ast::Expr::TsTypeAssertion(e) => self.expr_can_throw(&e.expr),
            swc_ast::Expr::TsSatisfies(e) => self.expr_can_throw(&e.expr),
            swc_ast::Expr::TsInstantiation(e) => self.expr_can_throw(&e.expr),
            _ => false,
        }
    }

    /// 表达式位置的异常检查分叉在 async / async-generator 函数体内会破坏其状态机的
    /// 基本块枚举与续延结构，故此类分叉仅在普通（非状态机）函数体及顶层代码中插入。
    /// async 函数体内的同步抛出沿用原有 promise rejection 路径（不在此处理）。
    pub(crate) fn expr_exception_fork_allowed(&self) -> bool {
        !self.is_async_fn && !self.is_async_generator_fn
    }
}

impl Lowerer {
    pub(crate) fn lower_expr_then_continue(
        &mut self,
        expr: &swc_ast::Expr,
        block: &mut BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let value = self.lower_expr(expr, *block)?;
        loop {
            let next = self.resolve_store_block(*block);
            if next == *block {
                break;
            }
            *block = next;
        }
        Ok(value)
    }

    pub(crate) fn lower_binary(
        &mut self,
        bin: &swc_ast::BinExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        use swc_ast::BinaryOp::*;

        match bin.op {
            // Logical operators — short circuit, may create new blocks
            LogicalAnd | LogicalOr | NullishCoalescing => self.lower_logical(bin, block),
            // Comparison operators
            EqEq | NotEq | EqEqEq | NotEqEq | Lt | LtEq | Gt | GtEq => {
                self.lower_comparison(bin, block)
            }
            // Standard arithmetic
            Add | Sub | Mul | Div => {
                let mut current_block = block;
                let lhs = self.lower_expr_then_continue(bin.left.as_ref(), &mut current_block)?;
                let rhs = self.lower_expr_then_continue(bin.right.as_ref(), &mut current_block)?;
                let dest = self.alloc_value();
                let op = match bin.op {
                    Add => BinaryOp::Add,
                    Sub => BinaryOp::Sub,
                    Mul => BinaryOp::Mul,
                    Div => BinaryOp::Div,
                    _ => unreachable!(),
                };
                self.current_function
                    .append_instruction(current_block, Instruction::Binary { dest, op, lhs, rhs });
                if current_block != block {
                    self.expr_merge_block = Some(current_block);
                }
                Ok(dest)
            }
            // Mod / Exp → Binary（后端按 BigInt / Number 分派）
            Mod | Exp => {
                let mut current_block = block;
                let lhs = self.lower_expr_then_continue(bin.left.as_ref(), &mut current_block)?;
                let rhs = self.lower_expr_then_continue(bin.right.as_ref(), &mut current_block)?;
                let dest = self.alloc_value();
                let op = if bin.op == Mod {
                    BinaryOp::Mod
                } else {
                    BinaryOp::Exp
                };
                self.current_function
                    .append_instruction(current_block, Instruction::Binary { dest, op, lhs, rhs });
                if current_block != block {
                    self.expr_merge_block = Some(current_block);
                }
                Ok(dest)
            }
            // Bitwise operators — convert to i32, operate, NaN-box back
            BitOr | BitXor | BitAnd | LShift | RShift | ZeroFillRShift => {
                let mut current_block = block;
                let lhs = self.lower_expr_then_continue(bin.left.as_ref(), &mut current_block)?;
                let rhs = self.lower_expr_then_continue(bin.right.as_ref(), &mut current_block)?;
                let dest = self.alloc_value();
                let op = match bin.op {
                    BitOr => BinaryOp::BitOr,
                    BitXor => BinaryOp::BitXor,
                    BitAnd => BinaryOp::BitAnd,
                    LShift => BinaryOp::Shl,
                    RShift => BinaryOp::Shr,
                    ZeroFillRShift => BinaryOp::UShr,
                    _ => unreachable!(),
                };
                self.current_function
                    .append_instruction(current_block, Instruction::Binary { dest, op, lhs, rhs });
                if current_block != block {
                    self.expr_merge_block = Some(current_block);
                }
                Ok(dest)
            }
            // in 操作符：检查对象是否有属性
            In => {
                let mut current_block = block;
                let prop = self.lower_expr_then_continue(bin.left.as_ref(), &mut current_block)?;
                let object =
                    self.lower_expr_then_continue(bin.right.as_ref(), &mut current_block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::In,
                        args: vec![object, prop],
                    },
                );
                if current_block != block {
                    self.expr_merge_block = Some(current_block);
                }
                Ok(dest)
            }
            // instanceof 操作符：检查原型链
            InstanceOf => {
                let mut current_block = block;
                let value = self.lower_expr_then_continue(bin.left.as_ref(), &mut current_block)?;
                let constructor =
                    self.lower_expr_then_continue(bin.right.as_ref(), &mut current_block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::InstanceOf,
                        args: vec![value, constructor],
                    },
                );
                if current_block != block {
                    self.expr_merge_block = Some(current_block);
                }
                Ok(dest)
            }
        }
    }

    /// Lower comparison operators → Compare instruction.
    /// 注意: == 和 != 使用 abstract_eq builtin 而不是 Compare 指令
    pub(crate) fn lower_comparison(
        &mut self,
        bin: &swc_ast::BinExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let mut current_block = block;
        let lhs = self.lower_expr_then_continue(bin.left.as_ref(), &mut current_block)?;
        let rhs = self.lower_expr_then_continue(bin.right.as_ref(), &mut current_block)?;
        let dest = self.alloc_value();

        match bin.op {
            // == 使用 abstract_eq builtin
            swc_ast::BinaryOp::EqEq => {
                self.current_function.append_instruction(
                    current_block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::AbstractEq,
                        args: vec![lhs, rhs],
                    },
                );
            }
            // != 使用 abstract_eq builtin 然后 Not
            swc_ast::BinaryOp::NotEq => {
                let eq_result = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::CallBuiltin {
                        dest: Some(eq_result),
                        builtin: Builtin::AbstractEq,
                        args: vec![lhs, rhs],
                    },
                );
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Not,
                        value: eq_result,
                    },
                );
            }
            // < 使用 abstract_compare builtin
            swc_ast::BinaryOp::Lt => {
                self.current_function.append_instruction(
                    current_block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::AbstractCompare,
                        args: vec![lhs, rhs],
                    },
                );
            }
            // > 相当于 (rhs < lhs)
            swc_ast::BinaryOp::Gt => {
                self.current_function.append_instruction(
                    current_block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::AbstractCompare,
                        args: vec![rhs, lhs],
                    },
                );
            }
            // <= 相当于 NOT (rhs < lhs)
            swc_ast::BinaryOp::LtEq => {
                let cmp_result = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::CallBuiltin {
                        dest: Some(cmp_result),
                        builtin: Builtin::AbstractCompare,
                        args: vec![rhs, lhs],
                    },
                );
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Not,
                        value: cmp_result,
                    },
                );
            }
            // >= 相当于 NOT (lhs < rhs)
            swc_ast::BinaryOp::GtEq => {
                let cmp_result = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::CallBuiltin {
                        dest: Some(cmp_result),
                        builtin: Builtin::AbstractCompare,
                        args: vec![lhs, rhs],
                    },
                );
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Not,
                        value: cmp_result,
                    },
                );
            }
            swc_ast::BinaryOp::EqEqEq => {
                self.current_function.append_instruction(
                    current_block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::StrictEq,
                        args: vec![lhs, rhs],
                    },
                );
            }
            swc_ast::BinaryOp::NotEqEq => {
                let eq_result = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::CallBuiltin {
                        dest: Some(eq_result),
                        builtin: Builtin::StrictEq,
                        args: vec![lhs, rhs],
                    },
                );
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Not,
                        value: eq_result,
                    },
                );
            }
            _ => unreachable!("lower_comparison called with non-comparison op"),
        }

        if current_block != block {
            self.expr_merge_block = Some(current_block);
        }

        Ok(dest)
    }

    /// Lower logical operators `&&`, `||`, `??` with short-circuit CFG.
    /// The merge block receives a real Phi so expression-level control flow is explicit in IR.
    pub(crate) fn lower_logical(
        &mut self,
        bin: &swc_ast::BinExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let lhs = self.lower_expr(bin.left.as_ref(), block)?;
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
        let rhs_block = self.current_function.new_block();
        let merge = self.current_function.new_block();

        let condition = if matches!(bin.op, swc_ast::BinaryOp::NullishCoalescing) {
            let is_nullish = self.alloc_value();
            self.current_function.append_instruction(
                branch_block,
                Instruction::Unary {
                    dest: is_nullish,
                    op: UnaryOp::IsNullish,
                    value: lhs,
                },
            );
            is_nullish
        } else {
            lhs
        };

        let (true_block, false_block) = match bin.op {
            swc_ast::BinaryOp::LogicalAnd => (rhs_block, merge),
            swc_ast::BinaryOp::LogicalOr => (merge, rhs_block),
            swc_ast::BinaryOp::NullishCoalescing => (rhs_block, merge),
            _ => unreachable!(),
        };

        self.current_function.set_terminator(
            branch_block,
            Terminator::Branch {
                condition,
                true_block,
                false_block,
            },
        );

        let rhs = self.lower_expr(bin.right.as_ref(), rhs_block)?;
        let rhs_end = self.resolve_store_block(rhs_block);
        self.current_function
            .set_terminator(rhs_end, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: branch_block,
                        value: lhs,
                    },
                    PhiSource {
                        predecessor: rhs_end,
                        value: rhs,
                    },
                ],
            },
        );

        self.expr_merge_block = Some(merge);

        Ok(result)
    }

    // ── Unary operators ─────────────────────────────────────────────────────

    pub(crate) fn lower_unary(
        &mut self,
        unary: &swc_ast::UnaryExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        use swc_ast::UnaryOp::*;

        match unary.op {
            Bang => {
                let mut current_block = block;
                let value = self.lower_expr_then_continue(&unary.arg, &mut current_block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Not,
                        value,
                    },
                );
                Ok(dest)
            }
            Minus => {
                let mut current_block = block;
                let value = self.lower_expr_then_continue(&unary.arg, &mut current_block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Neg,
                        value,
                    },
                );
                Ok(dest)
            }
            Plus => {
                let mut current_block = block;
                let value = self.lower_expr_then_continue(&unary.arg, &mut current_block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Pos,
                        value,
                    },
                );
                Ok(dest)
            }
            Tilde => {
                let mut current_block = block;
                let value = self.lower_expr_then_continue(&unary.arg, &mut current_block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::BitNot,
                        value,
                    },
                );
                Ok(dest)
            }
            Void => {
                let mut current_block = block;
                let _ = self.lower_expr_then_continue(&unary.arg, &mut current_block)?;
                // void returns undefined
                let undef = self.module.add_constant(Constant::Undefined);
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Const {
                        dest,
                        constant: undef,
                    },
                );
                Ok(dest)
            }
            TypeOf => {
                if let swc_ast::Expr::Ident(ident) = unary.arg.as_ref() {
                    let name = ident.sym.to_string();
                    let has_module_alias = self.current_module_id.is_some_and(|module_id| {
                        self.static_namespace_import_objects
                            .contains_key(&(module_id, name.clone()))
                            || self.import_aliases.contains_key(&(module_id, name.clone()))
                    });
                    if !has_module_alias
                        && !self.eval_scope_bridge_active()
                        && name != "eval"
                        && !is_builtin_global(&name)
                        && let Err(msg) = self.scopes.lookup(&name)
                        && msg.starts_with("undeclared identifier")
                    {
                        let undef_const = self
                            .module
                            .add_constant(Constant::String("undefined".to_string()));
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest,
                                constant: undef_const,
                            },
                        );
                        return Ok(dest);
                    }
                }

                let mut current_block = block;
                let arg = self.lower_expr_then_continue(&unary.arg, &mut current_block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::TypeOf,
                        args: vec![arg],
                    },
                );
                Ok(dest)
            }
            Delete => {
                // delete 操作符
                match unary.arg.as_ref() {
                    // delete obj.prop → DeleteProp 指令
                    swc_ast::Expr::Member(member) => {
                        let mut current_block = block;
                        let object =
                            self.lower_expr_then_continue(&member.obj, &mut current_block)?;
                        let key = match &member.prop {
                            swc_ast::MemberProp::Ident(ident) => {
                                let key_str = ident.sym.to_string();
                                let key_const = self.module.add_constant(Constant::String(key_str));
                                let key_val = self.alloc_value();
                                self.current_function.append_instruction(
                                    current_block,
                                    Instruction::Const {
                                        dest: key_val,
                                        constant: key_const,
                                    },
                                );
                                key_val
                            }
                            swc_ast::MemberProp::Computed(computed) => {
                                self.lower_expr_then_continue(&computed.expr, &mut current_block)?
                            }
                            _ => {
                                return Err(self.error(
                                    member.span(),
                                    "delete only supports identifier or computed property keys",
                                ));
                            }
                        };
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            current_block,
                            Instruction::DeleteProp { dest, object, key },
                        );
                        Ok(dest)
                    }
                    // delete x 对变量总是返回 true（不能删除变量）
                    swc_ast::Expr::Ident(_) => {
                        let true_const = self.module.add_constant(Constant::Bool(true));
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest,
                                constant: true_const,
                            },
                        );
                        Ok(dest)
                    }
                    _ => Err(self.error(
                        unary.span(),
                        "delete only supports member expressions or identifiers",
                    )),
                }
            }
        }
    }

    // ── Update expression (++x, x++, --x, x--) ─────────────────────────────

    pub(crate) fn lower_update(
        &mut self,
        update: &swc_ast::UpdateExpr,
        mut block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        use swc_ast::UpdateOp;

        // ── Step 1: 确定存储目标类型并加载当前值 ──
        enum Target {
            Var {
                ir_name: String,
                name: String,
                kind: VarKind,
            },
            Captured(ValueId, ValueId), // env_val, key_val
            Member {
                obj: ValueId,
                key: ValueId,
            },
        }

        let target = match update.arg.as_ref() {
            swc_ast::Expr::Ident(ident) => {
                let name = ident.sym.to_string();
                let (scope_id, kind) = self
                    .scopes
                    .lookup_for_assign(&name)
                    .map_err(|msg| self.error(update.span(), msg))?;

                let binding = CapturedBinding::new(name.clone(), scope_id);
                if self.binding_belongs_to_current_function(&binding)
                    && self.is_shared_binding(&binding)
                {
                    return self.lower_update_shared_local(
                        update,
                        block,
                        format!("${scope_id}.{name}"),
                        &binding,
                    );
                }

                if !self.binding_belongs_to_current_function(&binding) {
                    self.record_capture(binding.clone());
                    let env_val = self.load_env_object(block);
                    let key_val = self.append_env_key_const(block, &binding);
                    Target::Captured(env_val, key_val)
                } else {
                    Target::Var {
                        ir_name: format!("${scope_id}.{name}"),
                        name,
                        kind,
                    }
                }
            }
            swc_ast::Expr::SuperProp(super_prop) => {
                return self.lower_update_super_prop(update, super_prop, block);
            }
            swc_ast::Expr::Member(member) => {
                let mut current_block = block;
                let obj = self.lower_expr_then_continue(&member.obj, &mut current_block)?;
                let key = match &member.prop {
                    swc_ast::MemberProp::Ident(ident) => {
                        let key_const = self
                            .module
                            .add_constant(Constant::String(ident.sym.to_string()));
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
                    _ => {
                        return Err(self.error(
                            update.span(),
                            "unsupported member property in update expression target",
                        ));
                    }
                };
                block = current_block;
                Target::Member { obj, key }
            }
            _ => {
                return Err(self.error(
                    update.span(),
                    "update expression only supports identifier or member expression operands",
                ));
            }
        };

        // 1. 读取当前值
        let old_val = self.alloc_value();
        match &target {
            Target::Var { ir_name, .. } => {
                self.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: old_val,
                        name: ir_name.clone(),
                    },
                );
            }
            Target::Captured(env_val, key_val) => {
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest: old_val,
                        object: *env_val,
                        key: *key_val,
                    },
                );
            }
            Target::Member { obj, key } => {
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest: old_val,
                        object: *obj,
                        key: *key,
                    },
                );
            }
        }

        // 2. 转换为 Number (ToNumber)
        let num_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Unary {
                dest: num_val,
                op: UnaryOp::Pos,
                value: old_val,
            },
        );

        // 3. 常量 1.0
        let one = self.module.add_constant(Constant::Number(1.0));
        let one_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: one_val,
                constant: one,
            },
        );

        // 4. 执行加法或减法
        let new_val = self.alloc_value();
        let op = match update.op {
            UpdateOp::PlusPlus => BinaryOp::Add,
            UpdateOp::MinusMinus => BinaryOp::Sub,
        };
        self.current_function.append_instruction(
            block,
            Instruction::Binary {
                dest: new_val,
                op,
                lhs: num_val,
                rhs: one_val,
            },
        );

        // 5. 写回 (StoreVar / SetProp / SetProp for captured)
        match target {
            Target::Var {
                ir_name,
                name,
                kind,
            } => {
                self.current_function.append_instruction(
                    block,
                    Instruction::StoreVar {
                        name: ir_name,
                        value: new_val,
                    },
                );
                let after_write_block =
                    self.append_eval_var_leak_if_needed(&name, kind, new_val, block)?;
                if after_write_block != block {
                    self.expr_merge_block = Some(after_write_block);
                }
            }
            Target::Captured(env_val, key_val) => {
                self.current_function.append_instruction(
                    block,
                    Instruction::SetProp {
                        object: env_val,
                        key: key_val,
                        value: new_val,
                    },
                );
            }
            Target::Member { obj, key } => {
                self.current_function.append_instruction(
                    block,
                    Instruction::SetProp {
                        object: obj,
                        key,
                        value: new_val,
                    },
                );
            }
        }

        Ok(if update.prefix { new_val } else { num_val })
    }

    pub(crate) fn append_update_math(
        &mut self,
        block: BasicBlockId,
        old_val: ValueId,
        update_op: swc_ast::UpdateOp,
    ) -> (ValueId, ValueId) {
        let num_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Unary {
                dest: num_val,
                op: UnaryOp::Pos,
                value: old_val,
            },
        );

        let one = self.module.add_constant(Constant::Number(1.0));
        let one_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: one_val,
                constant: one,
            },
        );

        let new_val = self.alloc_value();
        let op = match update_op {
            swc_ast::UpdateOp::PlusPlus => BinaryOp::Add,
            swc_ast::UpdateOp::MinusMinus => BinaryOp::Sub,
        };
        self.current_function.append_instruction(
            block,
            Instruction::Binary {
                dest: new_val,
                op,
                lhs: num_val,
                rhs: one_val,
            },
        );

        (num_val, new_val)
    }

    fn lower_update_shared_local(
        &mut self,
        update: &swc_ast::UpdateExpr,
        block: BasicBlockId,
        ir_name: String,
        binding: &CapturedBinding,
    ) -> Result<ValueId, LoweringError> {
        let branch_block = if self.current_function.block(block).is_some_and(|b| {
            b.instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Phi { .. }))
        }) {
            let new_branch = self.current_function.new_block();
            self.current_function
                .set_terminator(block, Terminator::Jump { target: new_branch });
            new_branch
        } else {
            block
        };

        let env_val = self.alloc_value();
        self.current_function.append_instruction(
            branch_block,
            Instruction::LoadVar {
                dest: env_val,
                name: self.shared_env_ir_name(),
            },
        );
        let undef_const = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            branch_block,
            Instruction::Const {
                dest: undef_val,
                constant: undef_const,
            },
        );
        let env_missing = self.alloc_value();
        self.current_function.append_instruction(
            branch_block,
            Instruction::Compare {
                dest: env_missing,
                op: CompareOp::StrictEq,
                lhs: env_val,
                rhs: undef_val,
            },
        );

        let local_block = self.current_function.new_block();
        let env_block = self.current_function.new_block();
        let merge = self.current_function.new_block();
        self.current_function.set_terminator(
            branch_block,
            Terminator::Branch {
                condition: env_missing,
                true_block: local_block,
                false_block: env_block,
            },
        );

        let local_old = self.alloc_value();
        self.current_function.append_instruction(
            local_block,
            Instruction::LoadVar {
                dest: local_old,
                name: ir_name.clone(),
            },
        );
        let (local_num, local_new) = self.append_update_math(local_block, local_old, update.op);
        self.current_function.append_instruction(
            local_block,
            Instruction::StoreVar {
                name: ir_name,
                value: local_new,
            },
        );
        let local_result = if update.prefix { local_new } else { local_num };
        self.current_function
            .set_terminator(local_block, Terminator::Jump { target: merge });

        let key_val = self.append_env_key_const(env_block, binding);
        let env_old = self.alloc_value();
        self.current_function.append_instruction(
            env_block,
            Instruction::GetProp {
                dest: env_old,
                object: env_val,
                key: key_val,
            },
        );
        let (env_num, env_new) = self.append_update_math(env_block, env_old, update.op);
        self.current_function.append_instruction(
            env_block,
            Instruction::SetProp {
                object: env_val,
                key: key_val,
                value: env_new,
            },
        );
        let env_result = if update.prefix { env_new } else { env_num };
        self.current_function
            .set_terminator(env_block, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: local_block,
                        value: local_result,
                    },
                    PhiSource {
                        predecessor: env_block,
                        value: env_result,
                    },
                ],
            },
        );
        self.expr_merge_block = Some(merge);
        Ok(result)
    }
}
