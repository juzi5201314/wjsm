//! intrinsic 调用快路径的可变属性守卫编排。
//!
//! 快路径（`CallBuiltin` 直连宿主内建）只有在站点对应的 intrinsic 属性仍
//! 处于原始状态时才与规范语义一致；赋值、delete、defineProperty 访问器与
//! 全局名运行时遮蔽都必须让调用退回「通用属性查找 + 动态调用」。守卫由
//! 宿主 `Builtin::IntrinsicPristine` 纯查询判定（无可观察副作用），本模块
//! 负责发射统一的分叉骨架：
//!
//! ```text
//! entry:      guard = IntrinsicPristine(family, ...)
//!             branch guard → fast_pre / slow_pre
//! slow_pre:   按属性语义解析 callee/this（getter 副作用先于实参求值，
//!             与 EvaluateCall 的求值顺序一致），异常就地分叉传播
//! fast_pre:   占位 undefined（快路径不读属性，无可观察副作用）
//! prep_merge: callee/this phi；实参按 ArgumentListEvaluation 求值一次
//!             branch guard → fast_call / slow_call
//! fast_call:  CallBuiltin 快形状（含 Promise 静态 / MathMax spread 特例）
//! slow_call:  Call / OptionalCall / FuncApply 动态调用
//! merge:      结果 phi
//! ```
//!
//! 实参只降低一次（在 prep_merge 之后共享），避免按分支复制实参 IR 造成
//! 嵌套站点的指数膨胀。

use super::*;

/// intrinsic 快路径站点的家族与慢路径解析方式。
pub(crate) enum IntrinsicCallSite<'a> {
    /// 裸全局标识符调用（`parseInt(...)`）：慢路径经 GlobalEnvGet 解析
    /// 全局绑定（缺失名 ReferenceError），this 恒 undefined。
    GlobalIdent { name: &'a str },
    /// 内建容器静态成员调用（`String.raw(...)`）：慢路径先 GlobalEnvGet
    /// 容器名，再通用取属性（getter 生效），this 为容器。`optional` 为
    /// `X.y?.(...)` 形态——pristine 时静态方法恒存在不短路，慢路径按
    /// 可选调用对 nullish callee 短路。
    StaticMember {
        object: &'a str,
        prop: &'a str,
        optional: bool,
    },
}

impl Lowerer {
    fn append_undefined(&mut self, block: BasicBlockId) -> ValueId {
        let constant = self.module.add_constant(Constant::Undefined);
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        dest
    }

    /// GlobalEnvGet（ResolveBinding + GetValue）：全局词法记录 → 全局对象
    /// 属性（含惰性内建合成），缺失名抛 ReferenceError；异常就地分叉传播。
    fn emit_global_env_get(
        &mut self,
        block: &mut BasicBlockId,
        name: &str,
    ) -> Result<ValueId, LoweringError> {
        let global = self.alloc_value();
        self.current_function.append_instruction(
            *block,
            Instruction::LoadVar {
                dest: global,
                name: "$0.$global".to_string(),
            },
        );
        let name_val = self.append_string_const(*block, name);
        let flags = self.const_val_i64(*block, 0);
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            *block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::GlobalEnvGet,
                args: vec![global, name_val, flags],
            },
        );
        *block = self.lower_value_exception_branch(*block, dest)?;
        Ok(dest)
    }

    /// 发射守卫调用并分叉出 fast/slow 前置块；返回 (guard, fast_pre, slow_pre)。
    fn emit_pristine_fork(
        &mut self,
        block: BasicBlockId,
        guard_args: Vec<ValueId>,
    ) -> (ValueId, BasicBlockId, BasicBlockId) {
        let guard = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(guard),
                builtin: Builtin::IntrinsicPristine,
                args: guard_args,
            },
        );
        let fast_pre = self.current_function.new_block();
        let slow_pre = self.current_function.new_block();
        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition: guard,
                true_block: fast_pre,
                false_block: slow_pre,
            },
        );
        (guard, fast_pre, slow_pre)
    }

    /// 静态成员 / 裸全局标识符调用的守卫版发射（直接与可选调用共用）。
    /// 快路径保持既有 `CallBuiltin` 形状；慢路径按属性语义解析后动态调用。
    pub(crate) fn lower_intrinsic_guarded_call(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
        builtin: Builtin,
        site: IntrinsicCallSite,
    ) -> Result<ValueId, LoweringError> {
        if builtin == Builtin::ConsoleLog && call.args.is_empty() {
            return Err(self.error(call.span, "console.log requires at least 1 argument"));
        }
        let has_spread = Self::call_args_have_spread(&call.args);
        let promise_static = matches!(
            builtin,
            Builtin::PromiseResolveStatic
                | Builtin::PromiseRejectStatic
                | Builtin::PromiseAll
                | Builtin::PromiseRace
                | Builtin::PromiseAllSettled
                | Builtin::PromiseAny
                | Builtin::PromiseWithResolvers
        );
        if has_spread && builtin != Builtin::MathMax && !promise_static {
            // 既有 spread 形状本就通用解析 callee（先于实参求值），已遵守
            // 属性语义，无需守卫。
            return self.lower_host_builtin_call_expr(call, block, builtin);
        }

        // 守卫实参与分叉。
        let entry = self.resolve_store_block(block);
        let (guard_args, optional) = match &site {
            IntrinsicCallSite::GlobalIdent { name } => {
                let family = self.const_val_i64(
                    entry,
                    wjsm_ir::constants::INTRINSIC_FAMILY_GLOBAL_IDENT,
                );
                let name_val = self.append_string_const(entry, name);
                (vec![family, name_val], false)
            }
            IntrinsicCallSite::StaticMember {
                object,
                prop,
                optional,
            } => {
                let family = self.const_val_i64(
                    entry,
                    wjsm_ir::constants::INTRINSIC_FAMILY_STATIC_MEMBER,
                );
                let object_val = self.append_string_const(entry, object);
                let prop_val = self.append_string_const(entry, prop);
                let wire = self.const_val_i64(entry, i64::from(builtin.wire_id()));
                (vec![family, object_val, prop_val, wire], *optional)
            }
        };
        let (guard, fast_pre, slow_pre) = self.emit_pristine_fork(entry, guard_args);

        // 慢路径前置：按 EvaluateCall 顺序先解析 callee/this（getter 副作用
        // 先于实参求值），异常就地分叉。
        let mut slow_block = slow_pre;
        let (slow_callee, slow_this) = match &site {
            IntrinsicCallSite::GlobalIdent { name } => {
                let callee = self.emit_global_env_get(&mut slow_block, name)?;
                let this_val = self.append_undefined(slow_block);
                (callee, this_val)
            }
            IntrinsicCallSite::StaticMember { object, prop, .. } => {
                let container = self.emit_global_env_get(&mut slow_block, object)?;
                let key_val = self.append_string_const(slow_block, prop);
                let callee = self.alloc_value();
                self.current_function.append_instruction(
                    slow_block,
                    Instruction::GetProp {
                        dest: callee,
                        object: container,
                        key: key_val,
                    },
                );
                slow_block = self.lower_value_exception_branch(slow_block, callee)?;
                (callee, container)
            }
        };

        // 快路径前置：无属性读取，占位 undefined 汇入 phi。
        let fast_placeholder = self.append_undefined(fast_pre);
        let prep_merge = self.current_function.new_block();
        self.current_function.set_terminator(
            fast_pre,
            Terminator::Jump { target: prep_merge },
        );
        self.current_function.set_terminator(
            slow_block,
            Terminator::Jump { target: prep_merge },
        );
        let callee_phi = self.alloc_value();
        self.current_function.append_instruction(
            prep_merge,
            Instruction::Phi {
                dest: callee_phi,
                sources: vec![
                    PhiSource {
                        predecessor: fast_pre,
                        value: fast_placeholder,
                    },
                    PhiSource {
                        predecessor: slow_block,
                        value: slow_callee,
                    },
                ],
            },
        );
        let this_phi = self.alloc_value();
        self.current_function.append_instruction(
            prep_merge,
            Instruction::Phi {
                dest: this_phi,
                sources: vec![
                    PhiSource {
                        predecessor: fast_pre,
                        value: fast_placeholder,
                    },
                    PhiSource {
                        predecessor: slow_block,
                        value: slow_this,
                    },
                ],
            },
        );

        // 实参按 ArgumentListEvaluation 求值一次，fast/slow 共享。
        let mut args_block = prep_merge;
        let mut plain_args = Vec::new();
        let mut spread_array = None;
        if has_spread {
            let (array, end_block) = self.lower_call_args_to_array(&call.args, args_block)?;
            args_block = end_block;
            spread_array = Some(array);
        } else {
            for arg in &call.args {
                plain_args
                    .push(self.lower_call_operand_then_continue(&arg.expr, &mut args_block)?);
            }
        }
        let args_block = self.resolve_store_block(args_block);

        // 二次按同一守卫值分叉执行。
        let fast_call = self.current_function.new_block();
        let slow_call = self.current_function.new_block();
        let merge = self.current_function.new_block();
        self.current_function.set_terminator(
            args_block,
            Terminator::Branch {
                condition: guard,
                true_block: fast_call,
                false_block: slow_call,
            },
        );

        let (fast_result, fast_end) = self.emit_intrinsic_fast_shape(
            builtin,
            promise_static,
            fast_call,
            &plain_args,
            spread_array,
        )?;
        self.current_function
            .set_terminator(fast_end, Terminator::Jump { target: merge });

        let (slow_result, slow_end) = self.emit_intrinsic_slow_call(
            slow_call,
            callee_phi,
            this_phi,
            &plain_args,
            spread_array,
            optional,
        )?;
        self.current_function
            .set_terminator(slow_end, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: fast_end,
                        value: fast_result,
                    },
                    PhiSource {
                        predecessor: slow_end,
                        value: slow_result,
                    },
                ],
            },
        );
        self.expr_merge_block = Some(merge);
        Ok(result)
    }

    /// 快分支：保持既有 `CallBuiltin` 形状（Promise 静态方法补 species
    /// 构造器位、MathMax spread 走数组变体、JSON.parse 哨兵就地重抛）。
    fn emit_intrinsic_fast_shape(
        &mut self,
        builtin: Builtin,
        promise_static: bool,
        block: BasicBlockId,
        plain_args: &[ValueId],
        spread_array: Option<ValueId>,
    ) -> Result<(ValueId, BasicBlockId), LoweringError> {
        let mut current = block;
        let (effective_builtin, call_args) = if promise_static {
            let constructor = self.append_undefined(current);
            let mut args = vec![constructor];
            if let Some(array) = spread_array {
                args.push(self.lower_call_array_element(array, 0, current));
            } else {
                args.extend(plain_args.iter().copied());
                if args.len() == 1 {
                    args.push(self.append_undefined(current));
                }
            }
            (builtin, args)
        } else if let Some(array) = spread_array {
            debug_assert_eq!(builtin, Builtin::MathMax);
            (Builtin::MathMaxArray, vec![array])
        } else {
            (builtin, plain_args.to_vec())
        };
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            current,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: effective_builtin,
                args: call_args,
            },
        );
        if matches!(builtin, Builtin::JsonParse) {
            current = self.lower_value_exception_branch(current, dest)?;
        }
        Ok((dest, current))
    }

    /// 慢分支：解析出的 callee 动态调用（可选形态对 nullish callee 短路）。
    fn emit_intrinsic_slow_call(
        &mut self,
        block: BasicBlockId,
        callee: ValueId,
        this_val: ValueId,
        plain_args: &[ValueId],
        spread_array: Option<ValueId>,
        optional: bool,
    ) -> Result<(ValueId, BasicBlockId), LoweringError> {
        if let Some(array) = spread_array {
            if !optional {
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::FuncApply,
                        args: vec![callee, this_val, array],
                    },
                );
                return Ok((dest, block));
            }
            // 可选 spread：nullish callee 短路为 undefined，否则 FuncApply。
            let is_nullish = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Unary {
                    dest: is_nullish,
                    op: UnaryOp::IsNullish,
                    value: callee,
                },
            );
            let nullish_block = self.current_function.new_block();
            let apply_block = self.current_function.new_block();
            let merge = self.current_function.new_block();
            self.current_function.set_terminator(
                block,
                Terminator::Branch {
                    condition: is_nullish,
                    true_block: nullish_block,
                    false_block: apply_block,
                },
            );
            let undefined = self.append_undefined(nullish_block);
            self.current_function
                .set_terminator(nullish_block, Terminator::Jump { target: merge });
            let applied = self.alloc_value();
            self.current_function.append_instruction(
                apply_block,
                Instruction::CallBuiltin {
                    dest: Some(applied),
                    builtin: Builtin::FuncApply,
                    args: vec![callee, this_val, array],
                },
            );
            self.current_function
                .set_terminator(apply_block, Terminator::Jump { target: merge });
            let result = self.alloc_value();
            self.current_function.append_instruction(
                merge,
                Instruction::Phi {
                    dest: result,
                    sources: vec![
                        PhiSource {
                            predecessor: nullish_block,
                            value: undefined,
                        },
                        PhiSource {
                            predecessor: apply_block,
                            value: applied,
                        },
                    ],
                },
            );
            return Ok((result, merge));
        }
        let dest = self.alloc_value();
        if optional {
            self.current_function.append_instruction(
                block,
                Instruction::OptionalCall {
                    dest,
                    callee,
                    this_val,
                    args: plain_args.to_vec(),
                },
            );
        } else {
            self.current_function.append_instruction(
                block,
                Instruction::Call {
                    dest: Some(dest),
                    callee,
                    this_val,
                    args: plain_args.to_vec(),
                },
            );
        }
        Ok((dest, block))
    }

    /// %String.prototype% / %Array.prototype% 方法调用的守卫版发射。
    /// receiver 先求值一次并作为守卫实参；快路径 `CallBuiltin(builtin,
    /// [receiver, args...])`，慢路径通用取属性（getter 生效）后动态调用。
    pub(crate) fn lower_intrinsic_guarded_proto_call(
        &mut self,
        family: i64,
        builtin: Builtin,
        member_expr: &swc_ast::MemberExpr,
        prop_name: &str,
        args: &[swc_ast::ExprOrSpread],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if Self::call_args_have_spread(args) {
            // spread 形状本就通用解析 callee（先于实参求值），已遵守属性语义。
            return self.emit_proto_builtin_call(builtin, member_expr, args, block);
        }
        let mut entry = block;
        let receiver =
            self.lower_call_operand_then_continue(member_expr.obj.as_ref(), &mut entry)?;
        let entry = self.resolve_store_block(entry);
        let family_val = self.const_val_i64(entry, family);
        let name_val = self.append_string_const(entry, prop_name);
        let wire = self.const_val_i64(entry, i64::from(builtin.wire_id()));
        let (guard, fast_pre, slow_pre) =
            self.emit_pristine_fork(entry, vec![family_val, receiver, name_val, wire]);

        let mut slow_block = slow_pre;
        let slow_callee =
            self.lower_member_expr_from_object(member_expr, receiver, &mut slow_block, false)?;
        slow_block = self.lower_value_exception_branch(slow_block, slow_callee)?;

        let fast_placeholder = self.append_undefined(fast_pre);
        let prep_merge = self.current_function.new_block();
        self.current_function
            .set_terminator(fast_pre, Terminator::Jump { target: prep_merge });
        self.current_function
            .set_terminator(slow_block, Terminator::Jump { target: prep_merge });
        let callee_phi = self.alloc_value();
        self.current_function.append_instruction(
            prep_merge,
            Instruction::Phi {
                dest: callee_phi,
                sources: vec![
                    PhiSource {
                        predecessor: fast_pre,
                        value: fast_placeholder,
                    },
                    PhiSource {
                        predecessor: slow_block,
                        value: slow_callee,
                    },
                ],
            },
        );

        let mut args_block = prep_merge;
        let mut plain_args = Vec::with_capacity(args.len());
        for arg in args {
            plain_args.push(self.lower_call_operand_then_continue(&arg.expr, &mut args_block)?);
        }
        let args_block = self.resolve_store_block(args_block);

        let fast_call = self.current_function.new_block();
        let slow_call = self.current_function.new_block();
        let merge = self.current_function.new_block();
        self.current_function.set_terminator(
            args_block,
            Terminator::Branch {
                condition: guard,
                true_block: fast_call,
                false_block: slow_call,
            },
        );

        let fast_result = self.alloc_value();
        let mut fast_args = Vec::with_capacity(plain_args.len() + 1);
        fast_args.push(receiver);
        fast_args.extend(plain_args.iter().copied());
        self.current_function.append_instruction(
            fast_call,
            Instruction::CallBuiltin {
                dest: Some(fast_result),
                builtin,
                args: fast_args,
            },
        );
        // 异常分叉紧跟 CallBuiltin 同块发射：array_inline 展开数组高阶函数时
        // 靠该形状定位语句级 catch 路径（回调抛异常须进入包围 try 的 catch）。
        let fast_end = self.lower_value_exception_branch(fast_call, fast_result)?;
        self.current_function
            .set_terminator(fast_end, Terminator::Jump { target: merge });

        let slow_result = self.alloc_value();
        self.current_function.append_instruction(
            slow_call,
            Instruction::Call {
                dest: Some(slow_result),
                callee: callee_phi,
                this_val: receiver,
                args: plain_args,
            },
        );
        self.current_function
            .set_terminator(slow_call, Terminator::Jump { target: merge });

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: fast_end,
                        value: fast_result,
                    },
                    PhiSource {
                        predecessor: slow_call,
                        value: slow_result,
                    },
                ],
            },
        );
        self.expr_merge_block = Some(merge);
        Ok(result)
    }
}
