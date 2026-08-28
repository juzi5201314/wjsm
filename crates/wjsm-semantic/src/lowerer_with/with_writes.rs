//! with 分派的标识符写入侧：赋值、复合赋值、update、裸写（解构 / for 头）。

use super::*;

/// with 写分派未命中时静态回退的类别。
pub(crate) enum WithWriteFallback {
    /// 静态可写绑定：按局部 / 捕获 / 迭代环境写入。
    Binding {
        scope_id: usize,
        kind: VarKind,
        /// 跨函数前向引用：写入前发射运行时 TdzCheck。
        tdz_runtime: bool,
    },
    /// eval 桥接自由名：EvalSetBinding（由宿主处理未声明 / 全局回退）。
    EvalEnv,
    /// 未声明（sloppy，with 仅存在于非严格代码）：隐式全局属性写入。
    ImplicitGlobal,
    /// const 重赋值：命中 with 对象时合法，未命中运行时 TypeError（§13.15.2）。
    ConstViolation,
    /// 具名函数表达式自身名字绑定：不可变；with 仅存在于非严格代码，
    /// 未命中时写入静默忽略（CreateImmutableBinding(name, false)）。
    FnExprName,
    /// 同函数直线 TDZ 写入：未命中运行时 ReferenceError。
    Tdz,
}

impl Lowerer {
    /// 判定 with 写未命中回退的处理类别。
    pub(crate) fn with_write_fallback_kind(&self, name: &str) -> WithWriteFallback {
        if self.fn_expr_name_binding(name).is_some() {
            return WithWriteFallback::FnExprName;
        }
        match self.lookup_binding_for_assign(name) {
            Ok((scope_id, kind)) => WithWriteFallback::Binding {
                scope_id,
                kind,
                tdz_runtime: false,
            },
            Err(msg) if msg.starts_with("undeclared identifier") => {
                if self.eval_scope_bridge_active() {
                    WithWriteFallback::EvalEnv
                } else {
                    WithWriteFallback::ImplicitGlobal
                }
            }
            Err(msg) if msg.starts_with("cannot reassign a const") => {
                WithWriteFallback::ConstViolation
            }
            Err(_) => match self.runtime_tdz_binding(name) {
                Some((scope_id, kind)) if !matches!(kind, VarKind::Const) => {
                    WithWriteFallback::Binding {
                        scope_id,
                        kind,
                        tdz_runtime: true,
                    }
                }
                _ => WithWriteFallback::Tdz,
            },
        }
    }

    /// 静态绑定写入（局部 / 共享 env / 迭代环境 / 外层函数捕获），
    /// 返回写入完成后的延续块。
    fn emit_static_binding_write(
        &mut self,
        block: BasicBlockId,
        binding: &CapturedBinding,
        value: ValueId,
        span: Span,
    ) -> Result<BasicBlockId, LoweringError> {
        // 脚本全局绑定不走捕获 env 链：按 SetMutableBinding 写宿主全局环境记录。
        if binding.scope_id == Some(0) && self.script_global_names.contains_key(&binding.name) {
            return self.emit_script_global_set(block, &binding.name, value);
        }
        if self.iteration_env_for_binding(binding).is_some()
            || self.binding_belongs_to_current_function(binding)
        {
            return self.store_binding_value(block, binding, value, span, true);
        }
        // 外层函数绑定：解析 owner env 后 SetProp 写入声明帧。
        self.record_capture(binding.clone());
        let start_env = self.load_env_object(block);
        let (owner_block, owner_env) = if self.captured_binding_at_env_depth_zero(binding) {
            (block, start_env)
        } else {
            self.resolve_env_binding_owner(block, start_env, binding)
        };
        let key = self.append_env_key_const(owner_block, binding);
        self.emit_set_prop(owner_block, owner_env, key, value);
        Ok(owner_block)
    }

    /// with 未命中时的静态回退写入。返回 `(延续块, 该路径是否 open)`。
    pub(crate) fn lower_with_static_write(
        &mut self,
        name: &str,
        span: Span,
        value: ValueId,
        block: BasicBlockId,
    ) -> Result<(BasicBlockId, bool), LoweringError> {
        match self.with_write_fallback_kind(name) {
            WithWriteFallback::Binding {
                scope_id,
                kind,
                tdz_runtime,
            } => {
                let binding = CapturedBinding::new(name, scope_id);
                let mut cursor = block;
                if tdz_runtime {
                    // 跨函数前向写入：SetMutableBinding 前按当前值做 TdzCheck。
                    let loaded = self.load_captured_binding(cursor, &binding)?;
                    self.resolve_expr_continuations(&mut cursor);
                    let (_, cont) = self.emit_tdz_check(cursor, loaded, name)?;
                    cursor = cont;
                }
                let store_block = self.emit_static_binding_write(cursor, &binding, value, span)?;
                let after = self.append_eval_var_leak_if_needed(name, kind, value, store_block)?;
                Ok((after, true))
            }
            WithWriteFallback::EvalEnv => {
                let after = self.append_eval_env_write(name, value, block)?;
                Ok((after, true))
            }
            WithWriteFallback::ImplicitGlobal => {
                // sloppy 隐式全局（§13.15.2 → PutValue 未解析引用）：写入全局对象。
                let global_obj = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: global_obj,
                        name: "$0.$global".to_string(),
                    },
                );
                let key = self.append_string_const(block, name);
                let result = self.emit_set_prop(block, global_obj, key, value);
                let cont = self.fork_with_dispatch_exception(block, result)?;
                Ok((cont, true))
            }
            WithWriteFallback::ConstViolation => {
                let _ = self.emit_runtime_error_throw(
                    block,
                    Builtin::TypeErrorConstructor,
                    "Assignment to constant variable.",
                )?;
                Ok((block, false))
            }
            // with 体恒为非严格代码：对自身名字的写入静默忽略。
            WithWriteFallback::FnExprName => Ok((block, true)),
            WithWriteFallback::Tdz => {
                let _ = self.emit_runtime_error_throw(
                    block,
                    Builtin::ReferenceErrorConstructor,
                    &format!("Cannot access '{name}' before initialization"),
                )?;
                Ok((block, false))
            }
        }
    }

    /// 按已解析的 with base 写回：命中 SetProp(base, name)，未命中静态回退。
    /// 返回两路合并后的延续块。
    pub(crate) fn lower_with_dispatch_write(
        &mut self,
        name: &str,
        span: Span,
        base: ValueId,
        value: ValueId,
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        let (miss, hit) = self.branch_on_with_base(base, block);

        // 命中：PutValue → [[Set]]（setter / proxy set 异常传播；with 恒 sloppy）。
        let key = self.append_string_const(hit, name);
        let set_result = self.emit_set_prop(hit, base, key, value);
        let hit_end = self.fork_with_dispatch_exception(hit, set_result)?;

        let (miss_end, miss_open) = self.lower_with_static_write(name, span, value, miss)?;

        let merge = self.current_function.new_block();
        self.current_function
            .set_terminator(hit_end, Terminator::Jump { target: merge });
        if miss_open {
            self.current_function
                .set_terminator(miss_end, Terminator::Jump { target: merge });
        }
        Ok(merge)
    }

    /// 裸标识符写入（解构叶 / for-in/of 头）：解析链 + 写分派。
    pub(crate) fn lower_with_bare_write(
        &mut self,
        name: &str,
        span: Span,
        value: ValueId,
        crossed: &[usize],
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        let (base, post) = self.lower_with_resolution_chain(name, crossed, block)?;
        self.lower_with_dispatch_write(name, span, base, value, post)
    }

    /// with 分派的标识符赋值：`x = e`、复合赋值与逻辑复合赋值。
    pub(crate) fn lower_with_ident_assign(
        &mut self,
        assign: &swc_ast::AssignExpr,
        ident: &swc_ast::Ident,
        crossed: &[usize],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let name = ident.sym.to_string();
        if assign.op == swc_ast::AssignOp::Assign {
            // §13.15.2：先 ResolveBinding（探测链，可触发 proxy has trap），
            // 再求 RHS，最后 PutValue 到已解析的基座。
            let (base, post) = self.lower_with_resolution_chain(&name, crossed, block)?;
            let mut rhs_block = post;
            let rhs = self.lower_expr_then_continue(assign.right.as_ref(), &mut rhs_block)?;
            let out = self.lower_with_dispatch_write(&name, assign.span, base, rhs, rhs_block)?;
            self.expr_merge_block = Some(out);
            return Ok(rhs);
        }

        if matches!(
            assign.op,
            swc_ast::AssignOp::AndAssign
                | swc_ast::AssignOp::OrAssign
                | swc_ast::AssignOp::NullishAssign
        ) {
            return self.lower_with_logical_assign(assign, ident, crossed, block);
        }

        // 算术/位运算复合赋值：单次链解析，读与写共用 base。
        let bin_op = assign_op_to_binary(assign.op)
            .ok_or_else(|| self.error(assign.span, "unsupported compound assignment operator"))?;
        let (base, old_val, read_out) = self.lower_with_read_resolved(ident, crossed, block)?;
        let mut rhs_block = read_out;
        let rhs = self.lower_expr_then_continue(assign.right.as_ref(), &mut rhs_block)?;
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            rhs_block,
            Instruction::Binary {
                dest,
                op: bin_op,
                lhs: old_val,
                rhs,
            },
        );
        let out = self.lower_with_dispatch_write(&name, assign.span, base, dest, rhs_block)?;
        self.expr_merge_block = Some(out);
        Ok(dest)
    }

    /// with 分派的逻辑复合赋值（`&&=` / `||=` / `??=`）：读一次、短路、
    /// 写回共用同一 base。
    fn lower_with_logical_assign(
        &mut self,
        assign: &swc_ast::AssignExpr,
        ident: &swc_ast::Ident,
        crossed: &[usize],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let name = ident.sym.to_string();
        let (base, loaded, read_out) = self.lower_with_read_resolved(ident, crossed, block)?;

        let assign_block = self.current_function.new_block();
        let merge = self.current_function.new_block();
        let condition = if matches!(assign.op, swc_ast::AssignOp::NullishAssign) {
            let is_nullish = self.alloc_value();
            self.current_function.append_instruction(
                read_out,
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
            read_out,
            Terminator::Branch {
                condition,
                true_block,
                false_block,
            },
        );

        let mut rhs_block = assign_block;
        let rhs = self.lower_expr_then_continue(assign.right.as_ref(), &mut rhs_block)?;
        let write_out = self.lower_with_dispatch_write(&name, assign.span, base, rhs, rhs_block)?;
        self.current_function
            .set_terminator(write_out, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: read_out,
                        value: loaded,
                    },
                    PhiSource {
                        predecessor: write_out,
                        value: rhs,
                    },
                ],
            },
        );
        // Phi 块不再挂 Branch（CFG codegen 契约），插入续接块。
        let post = self.current_function.new_block();
        self.current_function
            .set_terminator(merge, Terminator::Jump { target: post });
        self.expr_merge_block = Some(post);
        Ok(result)
    }

    /// with 分派的 update 表达式（`x++` / `--x` 等）：读一次、ToNumeric、
    /// ±1、写回共用同一 base。
    pub(crate) fn lower_with_update(
        &mut self,
        update: &swc_ast::UpdateExpr,
        ident: &swc_ast::Ident,
        crossed: &[usize],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let name = ident.sym.to_string();
        let (base, old_val, read_out) = self.lower_with_read_resolved(ident, crossed, block)?;
        let (num_val, new_val, math_block) =
            self.append_update_math(read_out, old_val, update.op)?;
        let out = self.lower_with_dispatch_write(&name, update.span, base, new_val, math_block)?;
        self.expr_merge_block = Some(out);
        Ok(if update.prefix { new_val } else { num_val })
    }
}
