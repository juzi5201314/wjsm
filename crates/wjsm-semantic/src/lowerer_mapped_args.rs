//! mapped arguments 的 [[ParameterMap]] 形参别名重定向（ES §10.4.4）。
//!
//! 模型：映射期间形参绑定真值就是 arguments 对象的自有索引属性——
//! `arguments[i]` 的普通读写天然与绑定同步；形参名的读写在降级期改经
//! `MappedArgumentsBindingRead/Write` builtin 重定向到该对象，defineProperty
//! 降级 / delete / freeze 在宿主侧解除映射并把绑定值快照进侧表槽，此后
//! 形参与属性各自独立演化。
//!
//! 别名仅在满足全部条件时启用：非严格、非箭头、简单参数列表（无默认值/
//! rest/解构）、函数体引用 `arguments`、且函数闭包子树不含 direct eval
//! （eval 经激活记录读写局部槽位，与重定向不相容，命中时保持既有行为）。

use super::*;

/// mapped arguments 形参别名：重定向一次读写所需的全部静态信息。
#[derive(Debug, Clone)]
pub(crate) struct MappedArgAlias {
    /// 形参位置，即 arguments 对象上对应的索引属性名。
    pub(crate) index: u32,
    /// 持有 arguments 对象的合成隐藏绑定：用户代码无法命名（名字含 `.`），
    /// 重赋 `arguments` 标识符不影响别名基座；可被嵌套闭包按常规机制捕获。
    pub(crate) hidden: CapturedBinding,
}

impl Lowerer {
    /// 合成隐藏绑定名：`.` 不是合法标识符字符，与用户绑定永不冲突。
    pub(crate) const MAPPED_ARGS_HIDDEN: &'static str = "$args.mapped";

    /// 查询绑定是否为当前已登记的 mapped arguments 形参别名。
    pub(crate) fn mapped_arg_alias(&self, binding: &CapturedBinding) -> Option<MappedArgAlias> {
        if self.mapped_arg_aliases.is_empty() {
            return None;
        }
        let scope_id = binding.scope_id?;
        self.mapped_arg_aliases
            .get(&(scope_id, binding.name.clone()))
            .cloned()
    }

    /// 在各函数降级点（`emit_arguments_init` 之前）暂存别名元数据：
    /// 简单参数列表时记录用户形参 IR 名的有序列表，并扫描 direct eval。
    /// 只在 wrapper/body 谓词（严格性、is_arrow、is_method）必然一致的
    /// 降级点调用；未调用的降级点保持既有（无别名）行为。
    pub(crate) fn stage_arguments_alias_meta(
        &mut self,
        function: &swc_ast::Function,
        param_ir_names: &[String],
    ) {
        let all_simple = function
            .params
            .iter()
            .all(|param| matches!(param.pat, swc_ast::Pat::Ident(_)));
        self.arguments_simple_param_ir_names = all_simple
            .then(|| param_ir_names.get(2..).unwrap_or_default().to_vec())
            .filter(|names| !names.is_empty());
        self.arguments_alias_blocked = all_simple
            && function
                .body
                .as_ref()
                .is_some_and(|body| body.stmts.iter().any(stmt_subtree_may_call_eval));
    }

    /// 登记形参别名并把 arguments 对象存入隐藏绑定。声明失败（同函数重复
    /// 物化，不变量破坏）时放弃别名，保持普通 mapped 行为（宿主侧表沦为
    /// 惰性数据，无任何可观测差异）。
    pub(crate) fn register_mapped_arg_aliases(
        &mut self,
        block: BasicBlockId,
        param_ir_names: &[String],
        args_obj: ValueId,
    ) {
        let Ok(scope_id) = self
            .scopes
            .declare(Self::MAPPED_ARGS_HIDDEN, VarKind::Let, true)
        else {
            return;
        };
        let hidden = CapturedBinding::new(Self::MAPPED_ARGS_HIDDEN, scope_id);
        self.current_function.append_instruction(
            block,
            Instruction::StoreVar {
                name: hidden.var_ir_name(),
                value: args_obj,
            },
        );
        for (index, ir_name) in param_ir_names.iter().enumerate() {
            let binding = crate::lowerer_modules::parse_ir_name_to_binding(ir_name);
            let Some(param_scope) = binding.scope_id else {
                continue;
            };
            // 重复形参（sloppy 后者胜）：早出现槽已改名为临时名，用户代码
            // 解析不到，其别名条目天然死亡；同名键后插覆盖 → 名字命中最后
            // 一次出现，与 §10.4.4.7 “仅最后一次出现入 map” 一致。
            self.mapped_arg_aliases.insert(
                (param_scope, binding.name),
                MappedArgAlias {
                    index: index as u32,
                    hidden: hidden.clone(),
                },
            );
        }
    }

    /// 读取别名基座（arguments 对象）。隐藏绑定 write-once：本帧局部槽永远
    /// 持有对象引用（共享 env 持同一引用的副本），直读即可；嵌套闭包经捕获
    /// 链取值（record_capture + env 原型链 GetProp，直线代码，无分叉）。
    fn load_mapped_args_base(
        &mut self,
        block: BasicBlockId,
        alias: &MappedArgAlias,
    ) -> Result<ValueId, LoweringError> {
        if !self.binding_belongs_to_current_function(&alias.hidden) {
            return self.load_captured_binding(block, &alias.hidden);
        }
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest,
                name: alias.hidden.var_ir_name(),
            },
        );
        Ok(dest)
    }

    fn append_index_const(&mut self, block: BasicBlockId, index: u32) -> ValueId {
        let constant = self.module.add_constant(Constant::Number(f64::from(index)));
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        dest
    }

    /// 形参绑定读取：MappedArgumentsBindingRead(args, index)。直线代码。
    pub(crate) fn lower_mapped_arg_read(
        &mut self,
        block: BasicBlockId,
        alias: &MappedArgAlias,
    ) -> Result<ValueId, LoweringError> {
        let base = self.load_mapped_args_base(block, alias)?;
        let index_val = self.append_index_const(block, alias.index);
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::MappedArgumentsBindingRead,
                args: vec![base, index_val],
            },
        );
        Ok(dest)
    }

    /// 形参绑定写入：MappedArgumentsBindingWrite(args, index, value)。
    /// 直线代码，返回原块。
    pub(crate) fn emit_mapped_arg_write(
        &mut self,
        block: BasicBlockId,
        alias: &MappedArgAlias,
        value: ValueId,
    ) -> Result<BasicBlockId, LoweringError> {
        let base = self.load_mapped_args_base(block, alias)?;
        let index_val = self.append_index_const(block, alias.index);
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::MappedArgumentsBindingWrite,
                args: vec![base, index_val, value],
            },
        );
        Ok(block)
    }

    /// 别名形参的赋值（=、算术/位复合、逻辑复合），镜像局部绑定路径的求值
    /// 顺序：复合先读旧值再求 RHS；逻辑复合按短路 CFG 分叉。
    pub(crate) fn lower_assign_mapped_arg(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        alias: &MappedArgAlias,
    ) -> Result<ValueId, LoweringError> {
        match assign.op {
            swc_ast::AssignOp::Assign => {
                let rhs = self.lower_expr(assign.right.as_ref(), block)?;
                let store_block = self.resolve_store_block(block);
                let store_block = self.emit_mapped_arg_write(store_block, alias, rhs)?;
                self.expr_merge_block = Some(store_block);
                Ok(rhs)
            }
            swc_ast::AssignOp::AndAssign
            | swc_ast::AssignOp::OrAssign
            | swc_ast::AssignOp::NullishAssign => {
                self.lower_logical_assign_mapped_arg(assign, block, alias)
            }
            op => {
                let bin_op = assign_op_to_binary(op).ok_or_else(|| {
                    self.error(assign.span, "unsupported compound assignment operator")
                })?;
                let loaded = self.lower_mapped_arg_read(block, alias)?;
                let mut rhs_block = block;
                let rhs = self.lower_expr_then_continue(assign.right.as_ref(), &mut rhs_block)?;
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    rhs_block,
                    Instruction::Binary {
                        dest,
                        op: bin_op,
                        lhs: loaded,
                        rhs,
                    },
                );
                let store_block = self.emit_mapped_arg_write(rhs_block, alias, dest)?;
                self.expr_merge_block = Some(store_block);
                Ok(dest)
            }
        }
    }

    /// 别名形参的逻辑复合赋值（&&=、||=、??=）：短路时不写回。
    fn lower_logical_assign_mapped_arg(
        &mut self,
        assign: &swc_ast::AssignExpr,
        block: BasicBlockId,
        alias: &MappedArgAlias,
    ) -> Result<ValueId, LoweringError> {
        let loaded = self.lower_mapped_arg_read(block, alias)?;
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
            swc_ast::AssignOp::AndAssign | swc_ast::AssignOp::NullishAssign => {
                (assign_block, merge)
            }
            swc_ast::AssignOp::OrAssign => (merge, assign_block),
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
        let assign_end = self.emit_mapped_arg_write(assign_end, alias, rhs)?;
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

    /// 别名形参的 update 表达式（++/--）：读 → ToNumeric（异常先传播）→ ±1
    /// → 写回；前缀返回新值，后缀返回旧数值。
    pub(crate) fn lower_update_mapped_arg(
        &mut self,
        update: &swc_ast::UpdateExpr,
        block: BasicBlockId,
        alias: &MappedArgAlias,
    ) -> Result<ValueId, LoweringError> {
        let old_val = self.lower_mapped_arg_read(block, alias)?;
        let (num_val, new_val, math_block) = self.append_update_math(block, old_val, update.op)?;
        let store_block = self.emit_mapped_arg_write(math_block, alias, new_val)?;
        self.expr_merge_block = Some(store_block);
        Ok(if update.prefix { new_val } else { num_val })
    }
}

/// 语句子树是否可能包含 direct eval 调用（裸 `eval(...)`）。
///
/// 穿透一切嵌套函数：嵌套闭包内的 direct eval 同样能沿作用域链读写外层
/// 形参。保守以标识符文本判定（作用域内遮蔽的 `eval` 也算），误报只损失
/// 别名优化路径、不损失正确性。
fn stmt_subtree_may_call_eval(stmt: &swc_ast::Stmt) -> bool {
    use swc_core::ecma::visit::{Visit, VisitWith};
    struct EvalScan {
        found: bool,
    }
    impl Visit for EvalScan {
        fn visit_call_expr(&mut self, call: &swc_ast::CallExpr) {
            if let swc_ast::Callee::Expr(callee) = &call.callee
                && let swc_ast::Expr::Ident(ident) = callee.as_ref()
                && ident.sym.as_ref() == "eval"
            {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }
    }
    let mut scan = EvalScan { found: false };
    stmt.visit_with(&mut scan);
    scan.found
}
