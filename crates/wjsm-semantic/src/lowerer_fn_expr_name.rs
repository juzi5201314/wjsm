//! 具名函数表达式的自身名字绑定（§15.2.5 InstantiateOrdinaryFunctionExpression、
//! §15.5.5 / §15.6.4 / §15.8.4 的 generator / async / async generator 对应步骤）：
//!
//! 规范在外层词法环境与函数自身环境之间插入 funcEnv，
//! `CreateImmutableBinding(name, false)` 后于函数对象创建时 `InitializeBinding`。
//! funcEnv 按【每次求值】新建——同一表达式在循环中多次求值产生互不相干的
//! 绑定实例。本实现复用按迭代绑定的 env 帧机制：表达式求值点新建原型链接到
//! 外层词法环境的 env 对象，函数对象（闭包）创建后写为其自有属性，体内自
//! 引用沿闭包 env 原型链读到同一函数对象；随后弹出帧与作用域使名字对外不
//! 可见。
//!
//! 写入语义按 CreateImmutableBinding 的 S=false：非严格代码赋值静默忽略
//! （RHS 求值副作用保留），严格代码赋值在写点抛运行时 TypeError。

use super::*;

impl Lowerer {
    /// 解析 `name` 的最近可见绑定；若其为具名函数表达式自身名字绑定则返回。
    /// 同名内层声明（var / let / 形参）遮蔽时最近绑定不带标记，返回 None。
    pub(crate) fn fn_expr_name_binding(&self, name: &str) -> Option<CapturedBinding> {
        let scope_id = self.scopes.resolve_scope_id(name).ok()?;
        self.scopes
            .is_fn_expr_name(scope_id, name)
            .then(|| CapturedBinding::new(name, scope_id))
    }

    /// 进入具名函数表达式的名字作用域：压入块作用域、声明不可变绑定，并在
    /// 求值点新建仅含该绑定的 env 帧（funcEnv 按每次求值新建，循环内每轮
    /// 得到独立绑定实例）。`block` 就地推进到 env 帧建立后的延续块。
    pub(crate) fn begin_fn_expr_name_scope(
        &mut self,
        name: &str,
        block: &mut BasicBlockId,
        span: Span,
    ) -> Result<usize, LoweringError> {
        self.scopes.push_scope(ScopeKind::Block);
        let scope_id = self
            .scopes
            .declare(name, VarKind::Const, true)
            .map_err(|msg| self.error(span, msg))?;
        self.scopes
            .set_fn_expr_name(scope_id, name)
            .map_err(|msg| self.error(span, msg))?;
        let binding = CapturedBinding::new(name, scope_id);
        let (continuation, frame) = self.prepare_iteration_env(*block, vec![binding])?;
        self.initialize_empty_iteration_env(continuation, &frame);
        self.iteration_env_stack.push(frame);
        *block = continuation;
        Ok(scope_id)
    }

    /// 完成自身名字绑定初始化（InitializeBinding）：函数对象写为 funcEnv
    /// 的自有属性（体内自引用沿闭包 env 原型链读到同一对象），随后弹出
    /// env 帧与名字作用域，名字对外不可见。返回写入完成后的延续块。
    pub(crate) fn finish_fn_expr_name_scope(
        &mut self,
        block: BasicBlockId,
        name: &str,
        scope_id: usize,
        callee_val: ValueId,
    ) -> BasicBlockId {
        let binding = CapturedBinding::new(name, scope_id);
        let store_block = self.resolve_store_block(block);
        let env = self.load_iteration_env_for_binding(store_block, &binding);
        let key = self.append_env_key_const(store_block, &binding);
        self.emit_set_prop(store_block, env, key, callee_val);
        self.iteration_env_stack
            .pop()
            .expect("fn expr name env frame must be on stack");
        self.scopes.pop_scope();
        store_block
    }

    /// 发射不可变绑定写入违例：严格代码在写点构造 TypeError 并经异常分叉
    /// 传播（try/catch 可捕获），返回静态可达（动态不可达）的正常延续块；
    /// 非严格代码为无操作，原块继续。
    pub(crate) fn emit_fn_expr_name_write(
        &mut self,
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        if !self.strict_mode {
            return Ok(block);
        }
        let msg_val = self.append_string_const(block, "Assignment to constant variable.");
        let error_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(error_val),
                builtin: Builtin::TypeErrorConstructor,
                args: vec![msg_val],
            },
        );
        let exception_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(exception_val),
                builtin: Builtin::Throw,
                args: vec![error_val],
            },
        );
        self.lower_value_exception_branch(block, exception_val)
    }

    /// 对自身名字绑定的赋值表达式（§13.15.2 → PutValue 于不可变绑定）：
    /// 完整求值 RHS（含复合赋值的读-改与逻辑赋值的短路），仅写点分流。
    pub(crate) fn lower_assign_fn_expr_name(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> Result<ValueId, LoweringError> {
        match assign.op {
            swc_ast::AssignOp::Assign => {
                let mut current_block = block;
                let rhs =
                    self.lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
                let after = self.emit_fn_expr_name_write(current_block)?;
                self.expr_merge_block = Some(after);
                Ok(rhs)
            }
            swc_ast::AssignOp::AndAssign
            | swc_ast::AssignOp::OrAssign
            | swc_ast::AssignOp::NullishAssign => {
                self.lower_logical_assign_fn_expr_name(assign, block, binding)
            }
            op => {
                let bin_op = assign_op_to_binary(op).ok_or_else(|| {
                    self.error(assign.span, "unsupported compound assignment operator")
                })?;
                let old_val = self.load_captured_binding(block, binding)?;
                let mut current_block = self.resolve_store_block(block);
                let rhs =
                    self.lower_expr_then_continue(assign.right.as_ref(), &mut current_block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current_block,
                    Instruction::Binary {
                        dest,
                        op: bin_op,
                        lhs: old_val,
                        rhs,
                    },
                );
                let after = self.emit_fn_expr_name_write(current_block)?;
                self.expr_merge_block = Some(after);
                Ok(dest)
            }
        }
    }

    /// 自身名字绑定的逻辑复合赋值（&&= / ||= / ??=）：短路读旧值，
    /// 赋值分支求 RHS 后写点分流，Phi 合并表达式值。
    fn lower_logical_assign_fn_expr_name(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> Result<ValueId, LoweringError> {
        let loaded = self.load_captured_binding(block, binding)?;
        let branch_block = self.resolve_store_block(block);

        let assign_block = self.current_function.new_block();
        let merge = self.current_function.new_block();

        let condition = if matches!(assign.op, swc_ast::AssignOp::NullishAssign) {
            let is_nullish = self.alloc_value();
            self.current_function.append_instruction(
                branch_block,
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
            branch_block,
            Terminator::Branch {
                condition,
                true_block: true_target,
                false_block: false_target,
            },
        );

        let mut assign_end = assign_block;
        let rhs = self.lower_expr_then_continue(assign.right.as_ref(), &mut assign_end)?;
        let assign_end = self.emit_fn_expr_name_write(assign_end)?;
        self.current_function
            .set_terminator(assign_end, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: branch_block,
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

    /// 自身名字绑定的 update 表达式（§13.4 → PutValue 于不可变绑定）：
    /// 读旧值、ToNumeric（副作用与异常保留），写点分流后返回规范结果值。
    pub(crate) fn lower_update_fn_expr_name(
        &mut self,
        update: &swc_ast::UpdateExpr,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> Result<ValueId, LoweringError> {
        let old_val = self.load_captured_binding(block, binding)?;
        let current_block = self.resolve_store_block(block);
        let (num_val, new_val, math_block) =
            self.append_update_math(current_block, old_val, update.op)?;
        let after = self.emit_fn_expr_name_write(math_block)?;
        self.expr_merge_block = Some(after);
        Ok(if update.prefix { new_val } else { num_val })
    }
}
