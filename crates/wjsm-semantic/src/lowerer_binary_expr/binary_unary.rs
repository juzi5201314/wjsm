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
                // 松散相等与关系比较经 ToPrimitive 可调用用户 valueOf/toString/
                // @@toPrimitive 抛出（IsLooselyEqual / IsLessThan）；严格相等
                // （===/!==）无任何强制转换，不会自身抛出，走下方操作数递归。
                swc_ast::BinaryOp::EqEq
                | swc_ast::BinaryOp::NotEq
                | swc_ast::BinaryOp::Lt
                | swc_ast::BinaryOp::LtEq
                | swc_ast::BinaryOp::Gt
                | swc_ast::BinaryOp::GtEq => true,
                _ => {
                    self.expr_can_throw(bin.left.as_ref())
                        || self.expr_can_throw(bin.right.as_ref())
                }
            },
            swc_ast::Expr::Seq(seq) => seq.exprs.iter().any(|expr| self.expr_can_throw(expr)),
            // 条件表达式的结果 Phi 直接携带分支值：任一分支可抛时结果可能是异常哨兵。
            swc_ast::Expr::Cond(cond) => {
                self.expr_can_throw(cond.test.as_ref())
                    || self.expr_can_throw(cond.cons.as_ref())
                    || self.expr_can_throw(cond.alt.as_ref())
            }
            // 模板字面量：插值表达式可抛，且任意插值对象的 ToString 可能调用用户
            // toString 抛出（StringConcatVa 会把异常哨兵透传为结果），保守判定。
            swc_ast::Expr::Tpl(tpl) => !tpl.exprs.is_empty(),
            // -/+/~ 经 ToNumeric 可调用用户 valueOf/toString 抛出（Symbol/BigInt
            // 混用也抛 TypeError）；delete 成员可抛（严格模式不可配置属性、Proxy
            // trap）。!/void/typeof 自身不产出异常哨兵（ToBoolean/常量/typeof 表
            // 全定义），其操作数异常已在 lower_unary 内经操作数分叉传播。
            swc_ast::Expr::Unary(unary) => match unary.op {
                swc_ast::UnaryOp::Minus | swc_ast::UnaryOp::Plus | swc_ast::UnaryOp::Tilde => true,
                swc_ast::UnaryOp::Delete => {
                    matches!(unary.arg.as_ref(), swc_ast::Expr::Member(_))
                }
                _ => false,
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
}

impl Lowerer {
    pub(crate) fn lower_expr_then_continue(
        &mut self,
        expr: &swc_ast::Expr,
        block: &mut BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let value = self.lower_expr(expr, *block)?;
        while self.eval_continue_block.is_some()
            || self.new_expr_continue_block.is_some()
            || self.await_continue_block.is_some()
            || self.expr_merge_block.is_some()
        {
            let next = self.resolve_store_block(*block);
            if next != *block {
                *block = next;
            }
        }
        Ok(value)
    }

    /// 按 ArgumentListEvaluation / `? GetValue` 求值单个实参/操作数：求值后若该
    /// 表达式可能直接产生 TAG_EXCEPTION 则立即检查并传播异常。用于调用/构造
    /// 实参、方法 receiver、被调用者，以及二元运算的左右操作数（LHS 抛出须
    /// 短路 RHS 求值，异常哨兵不得作为普通值流入
    /// ApplyStringOrNumericBinaryOperator / IsLooselyEqual / IsLessThan）。
    pub(crate) fn lower_call_operand_then_continue(
        &mut self,
        expr: &swc_ast::Expr,
        block: &mut BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let value = self.lower_expr_then_continue(expr, block)?;
        if self.expr_can_throw(expr) {
            *block = self.lower_value_exception_branch(*block, value)?;
        }
        Ok(value)
    }

    /// 发射 `ArrayPushSpread` 并检查其返回值：GetIterator 对不可迭代值抛
    /// TypeError、迭代器 next()/value 读取抛错都会以 TAG_EXCEPTION 返回，
    /// 必须按 ECMAScript ArrayAccumulation / ArgumentListEvaluation 分叉传播，
    /// 不得丢弃后静默产生空数组。
    pub(crate) fn emit_array_push_spread_checked(
        &mut self,
        block: BasicBlockId,
        array: ValueId,
        source: ValueId,
    ) -> Result<BasicBlockId, LoweringError> {
        let result = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(result),
                builtin: Builtin::ArrayPushSpread,
                args: vec![array, source],
            },
        );
        self.lower_value_exception_branch(block, result)
    }

    /// 发射 `ObjectSpread` 并检查其结果：CopyDataProperties 读取 source 自有
    /// 属性时 getter/Proxy trap 抛错会以 TAG_EXCEPTION 返回，必须按
    /// ECMAScript CopyDataProperties 分叉传播，不得丢弃后静默产生残缺对象。
    pub(crate) fn emit_object_spread_checked(
        &mut self,
        block: BasicBlockId,
        object: ValueId,
        source: ValueId,
    ) -> Result<BasicBlockId, LoweringError> {
        let result = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::ObjectSpread {
                dest: result,
                object,
                source,
            },
        );
        self.lower_value_exception_branch(block, result)
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
            // ES §13.15.4 EvaluateStringOrNumericBinaryExpression：`? GetValue(lref)`
            // 抛出必须先传播并短路 RHS 求值，`? GetValue(rval)` 抛出必须先于
            // ApplyStringOrNumericBinaryOperator 传播——异常哨兵不得作为普通
            // 操作数流入 Binary（否则被字符串拼接/数值转换吞掉）。
            Add | Sub | Mul | Div => {
                let mut current_block = block;
                let lhs =
                    self.lower_call_operand_then_continue(bin.left.as_ref(), &mut current_block)?;
                let rhs =
                    self.lower_call_operand_then_continue(bin.right.as_ref(), &mut current_block)?;
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
            // Mod / Exp → Binary（后端按 BigInt / Number 分派）；操作数异常
            // 分叉语义同 Add 臂（GetValue 抛出先传播）。
            Mod | Exp => {
                let mut current_block = block;
                let lhs =
                    self.lower_call_operand_then_continue(bin.left.as_ref(), &mut current_block)?;
                let rhs =
                    self.lower_call_operand_then_continue(bin.right.as_ref(), &mut current_block)?;
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
            // Bitwise operators — convert to i32, operate, NaN-box back；操作数
            // 异常分叉语义同 Add 臂（GetValue 抛出先传播）。
            BitOr | BitXor | BitAnd | LShift | RShift | ZeroFillRShift => {
                let mut current_block = block;
                let lhs =
                    self.lower_call_operand_then_continue(bin.left.as_ref(), &mut current_block)?;
                let rhs =
                    self.lower_call_operand_then_continue(bin.right.as_ref(), &mut current_block)?;
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
            // in 操作符：检查对象是否有属性。
            // 左操作数为私有名（`#x in obj`）时按 ES §13.10.1 做 brand 检查：
            // 私有名在编译期解析为存储名，运行时经 PrivateHas 查实例/构造器的
            // 私有槽（字段/方法/访问器同一存储），RHS 非对象抛 TypeError，
            // 错误显示名（字段 `#x` / 实例方法访问器为类 brand）对齐 V8/Node。
            In => {
                if let swc_ast::Expr::PrivateName(private_name) = bin.left.as_ref() {
                    let (field_name, display_name) = self
                        .resolve_private_in_names(private_name.name.as_ref(), private_name.span)?;
                    let mut current_block = block;
                    let object =
                        self.lower_expr_then_continue(bin.right.as_ref(), &mut current_block)?;
                    // `? GetValue(rref)`：RHS 求值异常必须先传播，不得流入 brand 检查
                    // 被误报为 TypeError。
                    if self.expr_can_throw(bin.right.as_ref()) {
                        current_block = self.lower_value_exception_branch(current_block, object)?;
                    }
                    let key = self.emit_string_const(current_block, &field_name);
                    let display = self.emit_string_const(current_block, &display_name);
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        current_block,
                        Instruction::CallBuiltin {
                            dest: Some(dest),
                            builtin: Builtin::PrivateHas,
                            args: vec![object, key, display],
                        },
                    );
                    // brand 检查自身的 TypeError（receiver 非对象）须在本函数内分叉，
                    // 方法体内 try/catch 才能本地捕获。
                    let continue_block = self.lower_value_exception_branch(current_block, dest)?;
                    self.expr_merge_block = Some(continue_block);
                    return Ok(dest);
                }
                let mut current_block = block;
                let prop = self.lower_expr_then_continue(bin.left.as_ref(), &mut current_block)?;
                // ES §13.10.1 步骤 2 `? GetValue(lref)`：LHS 求值异常必须先传播并
                // 短路 RHS 求值，不得作为普通键值流入 HasProperty。
                if self.expr_can_throw(bin.left.as_ref()) {
                    current_block = self.lower_value_exception_branch(current_block, prop)?;
                }
                let object =
                    self.lower_expr_then_continue(bin.right.as_ref(), &mut current_block)?;
                // 步骤 4 `? GetValue(rref)`：RHS 求值异常传播，不得被吞掉返回 false。
                if self.expr_can_throw(bin.right.as_ref()) {
                    current_block = self.lower_value_exception_branch(current_block, object)?;
                }
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::In,
                        args: vec![object, prop],
                    },
                );
                // 步骤 5：RHS 非对象的 TypeError 与 Proxy has trap 异常须在本函数内
                // 分叉抛出，try/catch 才能本地捕获。
                let continue_block = self.lower_value_exception_branch(current_block, dest)?;
                self.expr_merge_block = Some(continue_block);
                Ok(dest)
            }
            // instanceof 操作符：检查原型链
            InstanceOf => {
                let mut current_block = block;
                let value = self.lower_expr_then_continue(bin.left.as_ref(), &mut current_block)?;
                // ES §13.10.1 步骤 2：LHS 求值异常先传播并短路 RHS 求值。
                if self.expr_can_throw(bin.left.as_ref()) {
                    current_block = self.lower_value_exception_branch(current_block, value)?;
                }
                let constructor =
                    self.lower_expr_then_continue(bin.right.as_ref(), &mut current_block)?;
                // 步骤 4：RHS 求值异常传播，不得被吞掉返回 false。
                if self.expr_can_throw(bin.right.as_ref()) {
                    current_block =
                        self.lower_value_exception_branch(current_block, constructor)?;
                }
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::InstanceOf,
                        args: vec![value, constructor],
                    },
                );
                // InstanceofOperator 自身的 TypeError（RHS 非对象/非可调用、非对象
                // prototype）与 @@hasInstance 用户码异常须分叉传播。
                let continue_block = self.lower_value_exception_branch(current_block, dest)?;
                self.expr_merge_block = Some(continue_block);
                Ok(dest)
            }
        }
    }

    /// Lower comparison operators → Compare instruction.
    /// 注意: == 和 != 使用 abstract_eq builtin 而不是 Compare 指令
    /// ES §13.10/§13.11：操作数 `? GetValue` 抛出先传播（LHS 抛错短路 RHS 求值）；
    /// 松散相等与关系比较自身经 ToPrimitive 可调用用户码抛出，结果须在本函数内
    /// 分叉，try/catch 才能本地捕获；严格相等（===/!==）无强制转换不自身抛出。
    /// 抑制上下文由 lower_expr_then_continue 的延迟分叉兜底（expr_can_throw 已含
    /// 松散/关系比较），async 状态机体内由宿主端透传异常哨兵。
    pub(crate) fn lower_comparison(
        &mut self,
        bin: &swc_ast::BinExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let mut current_block = block;
        let lhs = self.lower_call_operand_then_continue(bin.left.as_ref(), &mut current_block)?;
        let rhs = self.lower_call_operand_then_continue(bin.right.as_ref(), &mut current_block)?;
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
                current_block = self.lower_value_exception_branch(current_block, dest)?;
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
                // 分叉必须先于 Not：Not(异常哨兵) 会把异常折叠为布尔值丢失。
                current_block = self.lower_value_exception_branch(current_block, eq_result)?;
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Not,
                        value: eq_result,
                    },
                );
            }
            // 关系比较由 host owner 按原始左右顺序执行 ToPrimitive；reverse 只反转比较方向，
            // invert 表示 <=/>=，并保留 unordered(NaN/undefined) 必须返回 false 的语义。
            swc_ast::BinaryOp::Lt
            | swc_ast::BinaryOp::Gt
            | swc_ast::BinaryOp::LtEq
            | swc_ast::BinaryOp::GtEq => {
                let reverse = self.load_bool_constant(
                    matches!(bin.op, swc_ast::BinaryOp::Gt | swc_ast::BinaryOp::LtEq),
                    current_block,
                );
                let invert = self.load_bool_constant(
                    matches!(bin.op, swc_ast::BinaryOp::LtEq | swc_ast::BinaryOp::GtEq),
                    current_block,
                );
                self.current_function.append_instruction(
                    current_block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::AbstractCompare,
                        args: vec![lhs, rhs, reverse, invert],
                    },
                );
                current_block = self.lower_value_exception_branch(current_block, dest)?;
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
        // 左操作数抛出时必须中止整个逻辑表达式：异常哨兵的原始位恒为真值，
        // 直接作为 Branch 条件会让 `&&` 错误地继续求值右侧并丢失异常。
        let block = if self.expr_can_throw(bin.left.as_ref()) {
            self.lower_value_exception_branch(block, lhs)?
        } else {
            block
        };
        let branch_block = self.resolve_store_block(block);
        // 若 resolve_store_block 返回的 block 含 Phi（来自嵌套逻辑/条件表达式），
        // 不能直接在其上设置 Branch，否则同一 block 有 Phi + Branch，违反 CFG codegen 契约。
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

    fn publish_expr_continuation(&mut self, entry_block: BasicBlockId, continuation: BasicBlockId) {
        if continuation != entry_block {
            self.expr_merge_block = Some(continuation);
        }
    }

    pub(crate) fn lower_unary(
        &mut self,
        unary: &swc_ast::UnaryExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        use swc_ast::UnaryOp::*;

        match unary.op {
            Bang => {
                let mut current_block = block;
                let value =
                    self.lower_call_operand_then_continue(&unary.arg, &mut current_block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Not,
                        value,
                    },
                );
                self.publish_expr_continuation(block, current_block);
                Ok(dest)
            }
            Minus => {
                let mut current_block = block;
                let value =
                    self.lower_call_operand_then_continue(&unary.arg, &mut current_block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Neg,
                        value,
                    },
                );
                self.publish_expr_continuation(block, current_block);
                Ok(dest)
            }
            Plus => {
                let mut current_block = block;
                let value =
                    self.lower_call_operand_then_continue(&unary.arg, &mut current_block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Pos,
                        value,
                    },
                );
                self.publish_expr_continuation(block, current_block);
                Ok(dest)
            }
            Tilde => {
                let mut current_block = block;
                let value =
                    self.lower_call_operand_then_continue(&unary.arg, &mut current_block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::BitNot,
                        value,
                    },
                );
                self.publish_expr_continuation(block, current_block);
                Ok(dest)
            }
            Void => {
                let mut current_block = block;
                let _ = self.lower_call_operand_then_continue(&unary.arg, &mut current_block)?;
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
                self.publish_expr_continuation(block, current_block);
                Ok(dest)
            }
            TypeOf => {
                if let swc_ast::Expr::Ident(ident) = unary.arg.as_ref() {
                    // 解析穿越 with 作用域：typeof 须按对象环境记录动态分派。
                    let crossed = self.with_scopes_for_ident(ident.sym.as_ref());
                    if !crossed.is_empty() {
                        return self.lower_with_typeof(ident, &crossed, block);
                    }
                    let name = ident.sym.to_string();
                    let has_module_alias = self.current_module_id.is_some_and(|module_id| {
                        self.static_namespace_import_objects
                            .contains_key(&(module_id, name.clone()))
                            || self.import_aliases.contains_key(&(module_id, name.clone()))
                    });
                    if self.eval_scope_bridge_active() && self.scopes.lookup(&name).is_err() {
                        return self.lower_eval_typeof_binding(&name, block);
                    }
                    // 脚本全局绑定：typeof 经容忍读（可配置 var 属性可能已被
                    // delete，缺失返回 undefined；词法 TDZ 仍抛 ReferenceError）。
                    if self.script_global_kind_for(&name).is_some() {
                        let value = self.lower_script_global_read(block, &name, true)?;
                        let mut current_block = block;
                        self.resolve_expr_continuations(&mut current_block);
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            current_block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::TypeOf,
                                args: vec![value],
                            },
                        );
                        self.publish_expr_continuation(block, current_block);
                        return Ok(dest);
                    }
                    // Web 平台全局是可配置的真实全局属性：typeof 经容忍读
                    //（被 delete 后返回 "undefined" 而非 ReferenceError；
                    // 被改写后按新值分类）。
                    if !has_module_alias
                        && wjsm_ir::intrinsic_sites::web_global_property(&name).is_some()
                        && self.scopes.lookup(&name).is_err()
                    {
                        let value = self.lower_script_global_read(block, &name, true)?;
                        let mut current_block = block;
                        self.resolve_expr_continuations(&mut current_block);
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            current_block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: Builtin::TypeOf,
                                args: vec![value],
                            },
                        );
                        self.publish_expr_continuation(block, current_block);
                        return Ok(dest);
                    }
                    if !has_module_alias
                        && !self.eval_scope_bridge_active()
                        && name != "eval"
                        && !is_builtin_global(&name)
                        && let Err(msg) = self.scopes.lookup(&name)
                        && msg.starts_with("undeclared identifier")
                    {
                        // 脚本模式未声明名：typeof 须运行时解析（eval/vm 可能已
                        // 创建全局绑定），经 GlobalEnvGet 容忍缺失后 TypeOf。
                        if self.script_global_dynamic_free_name(&name) {
                            let value = self.lower_script_global_read(block, &name, true)?;
                            let mut current_block = block;
                            self.resolve_expr_continuations(&mut current_block);
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                current_block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: Builtin::TypeOf,
                                    args: vec![value],
                                },
                            );
                            self.publish_expr_continuation(block, current_block);
                            return Ok(dest);
                        }
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
                let arg = self.lower_call_operand_then_continue(&unary.arg, &mut current_block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::TypeOf,
                        args: vec![arg],
                    },
                );
                self.publish_expr_continuation(block, current_block);
                Ok(dest)
            }
            Delete => {
                // delete 操作符
                match unary.arg.as_ref() {
                    // delete obj.prop → DeleteProp 指令
                    swc_ast::Expr::Member(member) => {
                        let mut current_block = block;
                        // 对象/计算键求值抛出必须在 DeleteProp 前中止并传播。
                        let object =
                            self.lower_call_operand_then_continue(&member.obj, &mut current_block)?;
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
                            swc_ast::MemberProp::Computed(computed) => self
                                .lower_call_operand_then_continue(
                                    &computed.expr,
                                    &mut current_block,
                                )?,
                            _ => {
                                return Err(self.error(
                                    member.span(),
                                    "delete only supports identifier or computed property keys",
                                ));
                            }
                        };
                        let dest = self.alloc_value();
                        // strict 位随降级点静态确定：deleteStatus 为 false 时
                        // strict 抛 TypeError（§13.5.5.9 步骤 5.d）。
                        self.current_function.append_instruction(
                            current_block,
                            Instruction::DeleteProp {
                                dest,
                                object,
                                key,
                                strict: self.strict_mode,
                            },
                        );
                        self.publish_expr_continuation(block, current_block);
                        Ok(dest)
                    }
                    // delete x：绑定不可删除时返回 false，其余沿用既有恒 true。
                    // 严格代码中 delete 标识符是 early error（§13.5.1.1），
                    // 已在降级前由 strict_check 拒绝，此处只剩 sloppy 路径。
                    swc_ast::Expr::Ident(ident) => {
                        // 命中 with 对象环境记录时执行 [[Delete]]（§9.1.1.2.7）。
                        let crossed = self.with_scopes_for_ident(ident.sym.as_ref());
                        if !crossed.is_empty() {
                            return self.lower_with_delete(ident, &crossed, block);
                        }
                        // 脚本全局绑定 / 脚本模式未声明名：全局环境 DeleteBinding
                        // （词法与非可配置 var/函数属性返回 false；隐式全局可删）。
                        let name = ident.sym.to_string();
                        if self.script_global_kind_for(&name).is_some()
                            || self.script_global_dynamic_free_name(&name)
                        {
                            return self.lower_script_global_delete(block, &name);
                        }
                        // 函数环境的 `arguments` 绑定按 CreateMutableBinding(
                        // "arguments", false) 创建（§10.2.11 步骤 27/34），
                        // deletable=false：DeleteBinding 返回 false（§9.1.1.1.8）。
                        // 显式 var/形参名 arguments 同为不可删除的声明式绑定。
                        // 具名函数表达式自身名字与类自身名字按
                        // CreateImmutableBinding 创建，同样不可删除（§9.1.1.1.8 步骤 3）。
                        let deletable = !((name == "arguments"
                            && self.scopes.lookup(&name).is_ok())
                            || self.fn_expr_name_binding(&name).is_some()
                            || self.class_self_name_binding(&name).is_some());
                        let bool_const = self.module.add_constant(Constant::Bool(deletable));
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest,
                                constant: bool_const,
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
        let entry_block = block;

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

        // 跨函数前向 update（x++ 读取后声明的 let）：GetValue 处需要运行时 TdzCheck。
        let mut tdz_checked_name: Option<String> = None;
        let target = match update.arg.as_ref() {
            swc_ast::Expr::Ident(ident) => {
                // 解析穿越 with 作用域：读改写整链按对象环境记录动态分派。
                let crossed = self.with_scopes_for_ident(ident.sym.as_ref());
                if !crossed.is_empty() {
                    return self.lower_with_update(update, ident, &crossed, block);
                }
                let name = ident.sym.to_string();
                // 脚本全局绑定 / 脚本模式未声明名：读改写全链经 GlobalEnvGet/Set
                //（TDZ、const TypeError、缺失名 ReferenceError 均为运行时语义）。
                if self.script_global_kind_for(&name).is_some()
                    || self.script_global_dynamic_free_name(&name)
                {
                    return self.lower_script_global_update(update, block, &name);
                }
                // 具名函数表达式自身名字绑定：读改写在写点按不可变语义分流
                // （非严格静默忽略、严格 TypeError），先于 const 编译期拒绝。
                if let Some(binding) = self.fn_expr_name_binding(&name) {
                    return self.lower_update_fn_expr_name(update, block, &binding);
                }
                // 类自身名字绑定：受检读旧值（TDZ 内 ReferenceError）后在写点
                // 抛 TypeError，先于 const 编译期拒绝。
                if let Some(binding) = self.class_self_name_binding(&name) {
                    return self.lower_update_class_self_name(update, block, &binding);
                }
                let (scope_id, kind) = match self.lookup_binding_for_assign(&name) {
                    Ok(found) => found,
                    Err(msg) => {
                        let Some((scope_id, kind)) = self.runtime_tdz_binding(&name) else {
                            return Err(self.error(update.span(), msg));
                        };
                        if matches!(kind, VarKind::Const) {
                            return Err(self.error(update.span(), msg));
                        }
                        tdz_checked_name = Some(name.clone());
                        (scope_id, kind)
                    }
                };

                let binding = CapturedBinding::new(name.clone(), scope_id);
                // mapped arguments 形参别名：读改写整链落在 arguments 对象上。
                if let Some(alias) = self.mapped_arg_alias(&binding) {
                    return self.lower_update_mapped_arg(update, block, &alias);
                }
                if self.iteration_env_for_binding(&binding).is_some() {
                    let env = self.load_iteration_env_for_binding(block, &binding);
                    let key = self.append_env_key_const(block, &binding);
                    Target::Captured(env, key)
                } else if self.binding_belongs_to_current_function(&binding)
                    && self.is_shared_binding(&binding)
                {
                    return self.lower_update_shared_local(
                        update,
                        block,
                        format!("${scope_id}.{name}"),
                        &binding,
                    );
                } else if !self.binding_belongs_to_current_function(&binding) {
                    self.record_capture(binding.clone());
                    let start_env = self.load_env_object(block);
                    let (owner_block, owner_env) =
                        if self.captured_binding_at_env_depth_zero(&binding) {
                            // 深度 0 捕获：owner 就是 $env，跳过 has_own + get_proto_of 链查找
                            (block, start_env)
                        } else {
                            self.resolve_env_binding_owner(block, start_env, &binding)
                        };
                    block = owner_block;
                    let key_val = self.append_env_key_const(block, &binding);
                    Target::Captured(owner_env, key_val)
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
                // 对象/计算键求值抛出必须在属性读取前中止并传播。
                let obj = self.lower_call_operand_then_continue(&member.obj, &mut current_block)?;
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
                        self.lower_call_operand_then_continue(&computed.expr, &mut current_block)?
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
        let mut old_val = self.alloc_value();
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
                // 成员读取可触发用户 getter 抛出，必须在 ToNumeric 前中止传播。
                block = self.lower_value_exception_branch(block, old_val)?;
            }
        }
        if let Some(name) = &tdz_checked_name {
            let (checked, continue_block) = self.emit_tdz_check(block, old_val, name)?;
            old_val = checked;
            block = continue_block;
        }

        // 2–4. ToNumeric（抛出即中止、不得写回）后执行 ±1。
        let (num_val, new_val, math_block) = self.append_update_math(block, old_val, update.op)?;
        block = math_block;

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
                // owner 解析 / TDZ / ToNumeric 异常分叉都可能推进块；最终延续块
                // 必须上报，否则后续语句会误写已终结的入口块。
                let after_write_block =
                    self.append_eval_var_leak_if_needed(&name, kind, new_val, block)?;
                self.publish_expr_continuation(entry_block, after_write_block);
            }
            Target::Captured(env_val, key_val) => {
                let write_result = self.emit_set_prop(block, env_val, key_val, new_val);
                let continue_block = self.lower_value_exception_branch(block, write_result)?;
                // owner 解析后的 block 必须作为后续语句入口，不能再被入口 Jump 覆盖。
                self.expr_merge_block = Some(continue_block);
            }
            Target::Member { obj, key } => {
                let write_result = self.emit_set_prop(block, obj, key, new_val);
                let continue_block = self.lower_value_exception_branch(block, write_result)?;
                self.expr_merge_block = Some(continue_block);
            }
        }

        Ok(if update.prefix { new_val } else { num_val })
    }

    /// 发射 update 表达式的 ToNumeric 与 ±1，返回 (旧数值, 新数值, 延续块)。
    /// ToNumeric（UnaryOp::Pos）对对象操作数可调用用户 valueOf/toString 抛出，
    /// 必须在写回前检查并传播；证明为 Number 的热路径（循环计数器等）由
    /// typed_cfg 按值类分析折叠该分叉，不产生运行时代价。
    pub(crate) fn append_update_math(
        &mut self,
        block: BasicBlockId,
        old_val: ValueId,
        update_op: swc_ast::UpdateOp,
    ) -> Result<(ValueId, ValueId, BasicBlockId), LoweringError> {
        let num_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Unary {
                dest: num_val,
                op: UnaryOp::Pos,
                value: old_val,
            },
        );
        let block = self.lower_value_exception_branch(block, num_val)?;

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

        Ok((num_val, new_val, block))
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
        let (local_num, local_new, local_continue) =
            self.append_update_math(local_block, local_old, update.op)?;
        self.current_function.append_instruction(
            local_continue,
            Instruction::StoreVar {
                name: ir_name.clone(),
                value: local_new,
            },
        );
        let local_result = if update.prefix { local_new } else { local_num };
        self.current_function
            .set_terminator(local_continue, Terminator::Jump { target: merge });

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
        let (env_num, env_new, env_math_continue) =
            self.append_update_math(env_block, env_old, update.op)?;
        let write_result = self.emit_set_prop(env_math_continue, env_val, key_val, env_new);
        let env_continue = self.lower_value_exception_branch(env_math_continue, write_result)?;
        self.current_function.append_instruction(
            env_continue,
            Instruction::StoreVar {
                name: ir_name,
                value: env_new,
            },
        );
        let env_result = if update.prefix { env_new } else { env_num };
        self.current_function
            .set_terminator(env_continue, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: local_continue,
                        value: local_result,
                    },
                    PhiSource {
                        predecessor: env_continue,
                        value: env_result,
                    },
                ],
            },
        );
        self.expr_merge_block = Some(merge);
        Ok(result)
    }
}
