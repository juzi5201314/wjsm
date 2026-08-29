//! 可选链（`?.`）的链级降低（§13.3 OptionalExpression）。
//!
//! 每条链在最外层 `OptChainExpr` 建立一次「短路块 + 合并块」：链上任一
//! `?.` 环的基座 / callee 为 nullish 时跳链级短路块，链的**其余全部环**
//! （含后续非可选成员访问、调用与实参求值）一并跳过，合并块 phi 产出
//! undefined。环内访问一律发普通 GetProp / GetElem / Call——nullish 判定
//! 由分叉承担，短路值不会再流入后续环。逐环短路指令（OptionalGetProp
//! 产出 undefined 后继续喂给下一环）无法表达链级短路：`null?.a.b` 会让
//! undefined 流入 `.b` 触发 ToObject TypeError，`null?.[k()]` 会先求键。
//!
//! SWC 把链上每一环都包成 `OptChainExpr`（`optional` 标记该环是否带
//! `?.`），链内嵌套只出现在 `Member.obj` / `Call.callee` 位置；括号包裹
//! 的可选链是独立链（Paren 节点保留），经普通 `lower_expr` 自建短路
//! 结构，不共享本链短路块。

use super::*;
use crate::lowerer_calls_eval::IntrinsicCallSite;

/// 链级短路状态：`short_circuit` 是本链所有 `?.` 环共享的短路目标，
/// 惰性创建（快路径拦截可能吃掉唯一的 `?.` 环，不留孤儿块）。
struct OptionalChainState {
    short_circuit: Option<BasicBlockId>,
}

impl Lowerer {
    /// 降低整条可选链：最外层建短路/合并结构，环体递归共享短路块。
    pub(crate) fn lower_optchain(
        &mut self,
        oc: &swc_ast::OptChainExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 整链恰为 `X.y?.(...)` 单环（静态宿主 API）：保留守卫版 CallBuiltin
        // 直连快路径。其慢路径已按 §13.3.9.2 对 nullish callee 在实参求值前
        // 短路，链上没有其它 `?.` 环，逐环短路即链级短路。
        if oc.optional
            && let swc_ast::OptChainBase::Call(ocall) = oc.base.as_ref()
            && let swc_ast::Expr::Member(member_expr) = ocall.callee.as_ref()
            && let swc_ast::Expr::Ident(obj_ident) = member_expr.obj.as_ref()
            && let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
            && let Some(builtin) = builtin_from_static_member(&obj_ident.sym, &prop_ident.sym)
            && !self.global_intrinsic_shadowed(&obj_ident.sym)
            && self
                .with_scopes_for_ident(obj_ident.sym.as_ref())
                .is_empty()
        {
            let call_expr = swc_ast::CallExpr {
                span: ocall.span,
                ctxt: ocall.ctxt,
                callee: swc_ast::Callee::Expr(ocall.callee.clone()),
                args: ocall.args.to_vec(),
                type_args: ocall.type_args.clone(),
            };
            return self.lower_intrinsic_guarded_call(
                &call_expr,
                block,
                builtin,
                IntrinsicCallSite::StaticMember { optional: true },
            );
        }

        let mut state = OptionalChainState {
            short_circuit: None,
        };
        let mut current = block;
        let (value, _) = self.lower_optchain_link(oc, &mut current, &mut state)?;
        self.finish_optchain(block, current, value, state)
    }

    /// `delete` 作用于可选链（§13.5.1.2）：链短路时引用不存在，delete 直接
    /// 产出 true；最外环为成员访问时对其发 DeleteProp，为调用时引用已退化
    /// 为值——整链照常求值（副作用与异常保留），结果丢弃恒产出 true。
    pub(crate) fn lower_optchain_delete(
        &mut self,
        oc: &swc_ast::OptChainExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let mut state = OptionalChainState {
            short_circuit: None,
        };
        let mut current = block;
        let result = match oc.base.as_ref() {
            swc_ast::OptChainBase::Member(member) => {
                let object =
                    self.lower_optchain_base(member.obj.as_ref(), &mut current, &mut state)?;
                if oc.optional {
                    self.emit_optchain_nullish_branch(object, &mut current, &mut state);
                }
                let key = match &member.prop {
                    swc_ast::MemberProp::Ident(ident) => {
                        self.append_string_const(current, ident.sym.as_ref())
                    }
                    swc_ast::MemberProp::Computed(computed) => {
                        self.lower_call_operand_then_continue(&computed.expr, &mut current)?
                    }
                    swc_ast::MemberProp::PrivateName(name) => {
                        // §13.5.1.1 早错误：delete 不得作用于私有成员引用
                        // （V8 同口径文案）。
                        return Err(
                            self.error(name.span, "Private fields can not be deleted".to_string())
                        );
                    }
                };
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    current,
                    Instruction::DeleteProp {
                        dest,
                        object,
                        key,
                        strict: self.strict_mode,
                    },
                );
                // strict 下不可配置属性 / Proxy trap 抛出按 § 13.5.1.2 传播。
                current = self.lower_value_exception_branch(current, dest)?;
                dest
            }
            swc_ast::OptChainBase::Call(_) => {
                let (_, _) = self.lower_optchain_link(oc, &mut current, &mut state)?;
                self.append_bool_const(current, true)
            }
        };
        // 短路边产出 true：引用不是 Reference Record 时 delete 恒返回 true。
        match state.short_circuit {
            None => {
                self.publish_expr_continuation(block, current);
                Ok(result)
            }
            Some(short_circuit) => {
                let merge = self.current_function.new_block();
                self.current_function
                    .set_terminator(current, Terminator::Jump { target: merge });
                let true_val = self.append_bool_const(short_circuit, true);
                self.current_function
                    .set_terminator(short_circuit, Terminator::Jump { target: merge });
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    merge,
                    Instruction::Phi {
                        dest,
                        sources: vec![
                            PhiSource {
                                predecessor: current,
                                value: result,
                            },
                            PhiSource {
                                predecessor: short_circuit,
                                value: true_val,
                            },
                        ],
                    },
                );
                self.expr_merge_block = Some(merge);
                Ok(dest)
            }
        }
    }

    /// 链尾收束：有短路边时建合并块 phi（正常值 / 短路 undefined），
    /// 无短路边（快路径拦截吃掉唯一 `?.`）时直接以链值收束。
    fn finish_optchain(
        &mut self,
        entry: BasicBlockId,
        current: BasicBlockId,
        value: ValueId,
        state: OptionalChainState,
    ) -> Result<ValueId, LoweringError> {
        let Some(short_circuit) = state.short_circuit else {
            self.publish_expr_continuation(entry, current);
            return Ok(value);
        };
        let merge = self.current_function.new_block();
        self.current_function
            .set_terminator(current, Terminator::Jump { target: merge });
        let undefined = self.append_undefined_const(short_circuit);
        self.current_function
            .set_terminator(short_circuit, Terminator::Jump { target: merge });
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest,
                sources: vec![
                    PhiSource {
                        predecessor: current,
                        value,
                    },
                    PhiSource {
                        predecessor: short_circuit,
                        value: undefined,
                    },
                ],
            },
        );
        self.expr_merge_block = Some(merge);
        Ok(dest)
    }

    /// 降低链上一环，返回（环值, this 候选）。`block` 推进到环求值后的
    /// 延续块；this 候选仅成员环给出（随后的调用环按 EvaluateCall 以
    /// Reference base 绑定 this），调用环产出 None（调用结果再被调用时
    /// this 为 undefined）。
    fn lower_optchain_link(
        &mut self,
        oc: &swc_ast::OptChainExpr,
        block: &mut BasicBlockId,
        state: &mut OptionalChainState,
    ) -> Result<(ValueId, Option<ValueId>), LoweringError> {
        match oc.base.as_ref() {
            swc_ast::OptChainBase::Member(member) => {
                let object = self.lower_optchain_base(member.obj.as_ref(), block, state)?;
                if oc.optional {
                    self.emit_optchain_nullish_branch(object, block, state);
                }
                // 短路判定已由分叉完成：键求值（computed，§13.3.7.1 步骤在
                // 判定之后）与属性读取按普通成员访问发射。
                let value = self.lower_member_expr_from_object(member, object, block)?;
                // 属性读取（getter / Proxy trap / 私有品牌检查）抛出必须
                // 就地分叉传播，哨兵不得流入后续环的实参求值。
                *block = self.lower_value_exception_branch(*block, value)?;
                Ok((value, Some(object)))
            }
            swc_ast::OptChainBase::Call(ocall) => {
                let (callee, this_candidate) = self.lower_optchain_callee(ocall, block, state)?;
                if oc.optional {
                    self.emit_optchain_nullish_branch(callee, block, state);
                }
                let this_val = match this_candidate {
                    Some(value) => value,
                    None => self.append_undefined_const(*block),
                };
                // 实参求值在短路判定之后（§13.3.9.1 ArgumentListEvaluation），
                // nullish callee 不得触发实参副作用。
                let value = if Self::call_args_have_spread(&ocall.args) {
                    let (args_array, args_end) =
                        self.lower_call_args_to_array(&ocall.args, *block)?;
                    *block = args_end;
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        *block,
                        Instruction::CallBuiltin {
                            dest: Some(dest),
                            builtin: Builtin::FuncApply,
                            args: vec![callee, this_val, args_array],
                        },
                    );
                    dest
                } else {
                    let mut args = Vec::with_capacity(ocall.args.len());
                    for arg in &ocall.args {
                        args.push(self.lower_call_operand_then_continue(&arg.expr, block)?);
                    }
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        *block,
                        Instruction::Call {
                            dest: Some(dest),
                            callee,
                            this_val,
                            args,
                            // 链内调用位：callee 为链延续（`o.foo?.()`），
                            // 渲染器穿透 OptChain 包装（V8 链内节点无包装）。
                            callsite: Some(crate::callsite_render::render_call_callsite(
                                &ocall.callee,
                            )),
                        },
                    );
                    dest
                };
                // 调用抛出（含非 callable TypeError）就地分叉传播。
                *block = self.lower_value_exception_branch(*block, value)?;
                Ok((value, None))
            }
        }
    }

    /// 求链环的基座：链内嵌套环递归（共享短路块），其余表达式是链起点，
    /// 走普通求值（其中的独立可选链自建短路结构，不会误跳本链短路块）。
    fn lower_optchain_base(
        &mut self,
        expr: &swc_ast::Expr,
        block: &mut BasicBlockId,
        state: &mut OptionalChainState,
    ) -> Result<ValueId, LoweringError> {
        if let swc_ast::Expr::OptChain(inner) = expr {
            let (value, _) = self.lower_optchain_link(inner, block, state)?;
            Ok(value)
        } else {
            self.lower_call_operand_then_continue(expr, block)
        }
    }

    /// 求调用环的 callee 与 this 候选。`a.b?.()` 的 receiver 求值一次并
    /// 复用为 this（EvaluateCall 的 thisValue 取自 Reference base，
    /// `f().m?.()` 不得二次求值）；穿越 with 作用域的裸标识符按
    /// WithBaseObject 动态绑定 this（§9.1.1.2.10）。
    fn lower_optchain_callee(
        &mut self,
        ocall: &swc_ast::OptCall,
        block: &mut BasicBlockId,
        state: &mut OptionalChainState,
    ) -> Result<(ValueId, Option<ValueId>), LoweringError> {
        match ocall.callee.as_ref() {
            swc_ast::Expr::OptChain(inner) => self.lower_optchain_link(inner, block, state),
            swc_ast::Expr::Member(member) => {
                let object = self.lower_call_operand_then_continue(member.obj.as_ref(), block)?;
                let callee = self.lower_member_expr_from_object(member, object, block)?;
                // 方法查找（getter）抛出先于短路判定与实参求值传播，
                // 哨兵不得作为 callee 流入 Call。
                *block = self.lower_value_exception_branch(*block, callee)?;
                Ok((callee, Some(object)))
            }
            swc_ast::Expr::Ident(ident)
                if !self.with_scopes_for_ident(ident.sym.as_ref()).is_empty() =>
            {
                let crossed = self.with_scopes_for_ident(ident.sym.as_ref());
                let (callee, this_val, post) =
                    self.lower_with_callee_resolution(ident, &crossed, *block)?;
                *block = post;
                Ok((callee, Some(this_val)))
            }
            other => {
                let callee = self.lower_call_operand_then_continue(other, block)?;
                Ok((callee, None))
            }
        }
    }

    /// 发射 `?.` 短路分叉：值为 nullish 跳链级短路块，否则进延续块。
    /// 当前块含 Phi（嵌套逻辑/条件表达式的合并块）时先经 Jump 引出新块
    /// 再设 Branch——同一块 Phi + Branch 违反 CFG codegen 契约。
    fn emit_optchain_nullish_branch(
        &mut self,
        value: ValueId,
        block: &mut BasicBlockId,
        state: &mut OptionalChainState,
    ) {
        let branch_block = self.resolve_store_block(*block);
        let branch_block = if self.current_function.block(branch_block).is_some_and(|bb| {
            bb.instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Phi { .. }))
        }) {
            let next = self.current_function.new_block();
            self.current_function
                .set_terminator(branch_block, Terminator::Jump { target: next });
            next
        } else {
            branch_block
        };
        let is_nullish = self.alloc_value();
        self.current_function.append_instruction(
            branch_block,
            Instruction::Unary {
                dest: is_nullish,
                op: UnaryOp::IsNullish,
                value,
            },
        );
        let short_circuit = *state
            .short_circuit
            .get_or_insert_with(|| self.current_function.new_block());
        let continue_block = self.current_function.new_block();
        self.current_function.set_terminator(
            branch_block,
            Terminator::Branch {
                condition: is_nullish,
                true_block: short_circuit,
                false_block: continue_block,
            },
        );
        *block = continue_block;
    }
}
