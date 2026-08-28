use super::*;

impl Lowerer {
    /// 将调用实参按 ArgumentListEvaluation 收集为数组。
    /// 普通实参追加一个元素，spread 实参经 iterator 协议追加全部元素。
    /// 每个实参求值后立即检查异常哨兵：实参抛出必须中止调用并传播。
    pub(crate) fn lower_call_args_to_array(
        &mut self,
        args: &[swc_ast::ExprOrSpread],
        block: BasicBlockId,
    ) -> Result<(ValueId, BasicBlockId), LoweringError> {
        let array = self.alloc_value();
        let capacity = u32::try_from(args.len().max(4))
            .expect("call argument count must fit in array capacity");
        self.current_function.append_instruction(
            block,
            Instruction::NewArray {
                dest: array,
                capacity,
            },
        );

        let mut current = block;
        for arg in args {
            // ArgumentListEvaluation：实参求值抛异常必须传播，不得把
            // TAG_EXCEPTION 存入实参数组或让 spread 静默展开为空。
            let value = self.lower_call_operand_then_continue(&arg.expr, &mut current)?;
            if arg.spread.is_some() {
                current = self.emit_array_push_spread_checked(current, array, value)?;
            } else {
                self.current_function.append_instruction(
                    current,
                    Instruction::CallBuiltin {
                        dest: None,
                        builtin: Builtin::ArrayPush,
                        args: vec![array, value],
                    },
                );
            }
        }
        Ok((array, current))
    }

    pub(crate) fn call_args_have_spread(args: &[swc_ast::ExprOrSpread]) -> bool {
        args.iter().any(|arg| arg.spread.is_some())
    }
    pub(crate) fn lower_call_array_element(
        &mut self,
        array: ValueId,
        index: u32,
        block: BasicBlockId,
    ) -> ValueId {
        let index_val = self.const_val_i64(block, i64::from(index));
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::GetElem {
                dest,
                object: array,
                index: index_val,
            },
        );
        dest
    }
    /// 可选调用的短路发射：`callee?.(args)` 的短路点在 ArgumentListEvaluation
    /// 之前（§13.3.9.1），callee 为 nullish 时直接产出 undefined，实参一律
    /// 不求值。非 nullish 分支才求值实参：spread 形态经参数数组 + FuncApply，
    /// 非 spread 形态发 OptionalCall（由 backend 处理非可调用 TypeError）。
    fn lower_optional_call_short_circuit(
        &mut self,
        callee: ValueId,
        this_val: ValueId,
        args: &[swc_ast::ExprOrSpread],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let branch_block = self.resolve_store_block(block);
        let branch_block = if self.current_function.block(branch_block).is_some_and(|bb| {
            bb.instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Phi { .. }))
        }) {
            let next_block = self.current_function.new_block();
            self.current_function
                .set_terminator(branch_block, Terminator::Jump { target: next_block });
            next_block
        } else {
            branch_block
        };

        let is_nullish = self.alloc_value();
        self.current_function.append_instruction(
            branch_block,
            Instruction::Unary {
                dest: is_nullish,
                op: UnaryOp::IsNullish,
                value: callee,
            },
        );
        let nullish_block = self.current_function.new_block();
        let call_block = self.current_function.new_block();
        let merge_block = self.current_function.new_block();
        self.current_function.set_terminator(
            branch_block,
            Terminator::Branch {
                condition: is_nullish,
                true_block: nullish_block,
                false_block: call_block,
            },
        );

        let undefined = self.alloc_value();
        let undefined_constant = self.module.add_constant(Constant::Undefined);
        self.current_function.append_instruction(
            nullish_block,
            Instruction::Const {
                dest: undefined,
                constant: undefined_constant,
            },
        );
        self.current_function.set_terminator(
            nullish_block,
            Terminator::Jump {
                target: merge_block,
            },
        );

        let (called, args_end) = if Self::call_args_have_spread(args) {
            let (args_array, args_end) = self.lower_call_args_to_array(args, call_block)?;
            let called = self.alloc_value();
            self.current_function.append_instruction(
                args_end,
                Instruction::CallBuiltin {
                    dest: Some(called),
                    builtin: Builtin::FuncApply,
                    args: vec![callee, this_val, args_array],
                },
            );
            (called, args_end)
        } else {
            let mut args_end = call_block;
            let mut arg_vals = Vec::with_capacity(args.len());
            for arg in args {
                arg_vals.push(self.lower_call_operand_then_continue(&arg.expr, &mut args_end)?);
            }
            let called = self.alloc_value();
            self.current_function.append_instruction(
                args_end,
                Instruction::OptionalCall {
                    dest: called,
                    callee,
                    this_val,
                    args: arg_vals,
                },
            );
            (called, args_end)
        };
        self.current_function.set_terminator(
            args_end,
            Terminator::Jump {
                target: merge_block,
            },
        );

        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge_block,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: nullish_block,
                        value: undefined,
                    },
                    PhiSource {
                        predecessor: args_end,
                        value: called,
                    },
                ],
            },
        );
        self.expr_merge_block = Some(merge_block);
        Ok(result)
    }
    /// 原型方法拦截的公共发射逻辑：把 `obj.method(args...)` 降为
    /// `CallBuiltin(builtin, [this, args...])`，其中 this = obj。
    ///
    /// `lower_call_expr` 中多个拦截点（String/Object/Number/Boolean/
    /// SharedArrayBuffer/DataView 原型方法）共用这段「lower obj 为 this →
    /// lower 每个实参 → 追加 CallBuiltin → 返回 dest」样板，
    /// 拦截点只保留各自的「模式识别 + receiver guard」判定。
    pub(crate) fn emit_proto_builtin_call(
        &mut self,
        builtin: Builtin,
        member_expr: &swc_ast::MemberExpr,
        args: &[swc_ast::ExprOrSpread],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 实参求值可能引入控制流（闭包共享环境 phi、三元、异常分叉等），
        // 必须用延续块推进 call_block，否则 CallBuiltin 会发射在
        // 过时的入口块上、先于实参指令执行。
        let mut call_block = block;
        let this_val =
            self.lower_call_operand_then_continue(member_expr.obj.as_ref(), &mut call_block)?;
        let dest;
        if Self::call_args_have_spread(args) {
            let callee_val =
                self.lower_member_expr_from_object(member_expr, this_val, &mut call_block, false)?;
            let (args_array, end_block) = self.lower_call_args_to_array(args, call_block)?;
            call_block = end_block;
            dest = self.alloc_value();
            self.current_function.append_instruction(
                call_block,
                Instruction::CallBuiltin {
                    dest: Some(dest),
                    builtin: Builtin::FuncApply,
                    args: vec![callee_val, this_val, args_array],
                },
            );
        } else {
            let mut builtin_args = vec![this_val];
            for arg in args {
                builtin_args
                    .push(self.lower_call_operand_then_continue(&arg.expr, &mut call_block)?);
            }
            dest = self.alloc_value();
            self.current_function.append_instruction(
                call_block,
                Instruction::CallBuiltin {
                    dest: Some(dest),
                    builtin,
                    args: builtin_args,
                },
            );
        }
        if call_block != block {
            self.expr_merge_block = Some(call_block);
        }
        Ok(dest)
    }

    fn is_import_meta_resolve_member(member_expr: &swc_ast::MemberExpr) -> bool {
        matches!(
            member_expr.obj.as_ref(),
            swc_ast::Expr::MetaProp(meta)
                if matches!(meta.kind, swc_ast::MetaPropKind::ImportMeta)
        ) && matches!(
            &member_expr.prop,
            swc_ast::MemberProp::Ident(prop_ident) if prop_ident.sym.as_ref() == "resolve"
        )
    }

    fn lower_import_meta_resolve_call(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let metadata = self.import_meta_metadata(call.span())?;

        let filename_const = self
            .module
            .add_constant(Constant::String(metadata.filename));
        let filename_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: filename_val,
                constant: filename_const,
            },
        );
        let resolve_fn = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(resolve_fn),
                builtin: Builtin::ImportMetaResolve,
                args: vec![filename_val],
            },
        );

        let undef_const = self.module.add_constant(Constant::Undefined);
        let this_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: this_val,
                constant: undef_const,
            },
        );

        let mut call_block = block;
        let dest;
        if Self::call_args_have_spread(&call.args) {
            let (args_array, end_block) = self.lower_call_args_to_array(&call.args, call_block)?;
            call_block = end_block;
            dest = self.alloc_value();
            self.current_function.append_instruction(
                call_block,
                Instruction::CallBuiltin {
                    dest: Some(dest),
                    builtin: Builtin::FuncApply,
                    args: vec![resolve_fn, this_val, args_array],
                },
            );
        } else {
            let mut args = Vec::with_capacity(call.args.len());
            for arg in &call.args {
                args.push(self.lower_call_operand_then_continue(&arg.expr, &mut call_block)?);
            }
            dest = self.alloc_value();
            self.current_function.append_instruction(
                call_block,
                Instruction::Call {
                    dest: Some(dest),
                    callee: resolve_fn,
                    this_val,
                    args,
                },
            );
        }
        let continue_block = self.lower_value_exception_branch(call_block, dest)?;
        self.expr_merge_block = Some(continue_block);
        Ok(dest)
    }

    pub(crate) fn lower_call_expr(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 裸标识符 callee 解析穿越 with 作用域：callee/this 按对象环境记录
        // 动态分派（含 `eval` 名字被 with 对象遮蔽时退化为普通调用）。
        if let swc_ast::Callee::Expr(expr) = &call.callee
            && let swc_ast::Expr::Ident(ident) = expr.as_ref()
        {
            let crossed = self.with_scopes_for_ident(ident.sym.as_ref());
            if !crossed.is_empty() {
                return self.lower_with_ident_call(call, ident, &crossed, block);
            }
        }

        let callee_val: ValueId;
        let this_val: ValueId;
        let callee_block: BasicBlockId;

        match &call.callee {
            swc_ast::Callee::Expr(expr) => {
                if let swc_ast::Expr::Ident(ident) = expr.as_ref() {
                    if ident.sym.as_ref() == "eval" && self.scopes.lookup("eval").is_err() {
                        let (val, merge_block) = self.lower_direct_eval_call(call, block)?;
                        self.eval_continue_block = Some(merge_block);
                        return Ok(val);
                    }
                    // 词法/模块绑定（含导入别名与 TDZ 声明）遮蔽全局 intrinsic：
                    // 只有名字未被遮蔽时才允许 CallBuiltin 快路径；运行时的
                    // 赋值 / delete / defineProperty 遮蔽由 pristine 守卫分流。
                    if let Some(builtin) = builtin_from_global_ident(&ident.sym)
                        && !self.global_intrinsic_shadowed(&ident.sym)
                    {
                        return self.lower_intrinsic_guarded_call(
                            call,
                            block,
                            builtin,
                            IntrinsicCallSite::GlobalIdent,
                        );
                    }
                }

                if let swc_ast::Expr::SuperProp(super_prop) = expr.as_ref() {
                    // super.m(...)：receiver 是 GetThisBinding 结果（派生构造器
                    // this TDZ 检查可能分叉，块就地推进），方法查找复用同一值。
                    let mut super_block = block;
                    this_val = self.lower_this_checked(&mut super_block)?;
                    callee_val =
                        self.lower_super_prop_with_this(super_prop, this_val, &mut super_block)?;
                    // 方法查找（原型链 getter）抛出必须先于调用分叉传播，
                    // 哨兵不得作为 callee 流入 Call。
                    super_block = self.lower_value_exception_branch(super_block, callee_val)?;
                    callee_block = super_block;
                // 检测 MemberExpr 被调用者 → 提取 obj 作为 this
                } else if let swc_ast::Expr::Member(member_expr) = expr.as_ref() {
                    if Self::is_import_meta_resolve_member(member_expr) {
                        return self.lower_import_meta_resolve_call(call, block);
                    }

                    // 静态宿主 API（console.*, Object.*, JSON.*）不读取对象本身。
                    // 对象名被词法/模块绑定遮蔽或解析穿越 with 作用域时禁用
                    // （导入别名、TDZ 声明与 with 对象都可能提供同名值）；
                    // 运行时的赋值 / delete / getter 替换由 pristine 守卫分流，
                    // Promise 静态方法的 species 构造器位形状在守卫发射内保持。
                    if let swc_ast::Expr::Ident(obj_ident) = member_expr.obj.as_ref()
                        && let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
                        && let Some(builtin) =
                            builtin_from_static_member(&obj_ident.sym, &prop_ident.sym)
                        && (!self.global_intrinsic_shadowed(&obj_ident.sym)
                            || self.eval_scope_bridge_active())
                        && self
                            .with_scopes_for_ident(obj_ident.sym.as_ref())
                            .is_empty()
                    {
                        return self.lower_intrinsic_guarded_call(
                            call,
                            block,
                            builtin,
                            IntrinsicCallSite::StaticMember { optional: false },
                        );
                    }

                    // TypedArray.prototype 方法调用优化（必须在 String 之前，因为 at/indexOf/includes/toString
                    // 在 String 和 TypedArray 上同名）。仅在 receiver 是已知 TypedArray 绑定时启用，
                    // 避免错误拦截普通字符串的同名方法调用。
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
                        && let Some(ta_builtin) =
                            builtin_from_typedarray_proto_method(&prop_ident.sym)
                        && let swc_ast::Expr::Ident(receiver_ident) = member_expr.obj.as_ref()
                        && self.is_typedarray_binding(receiver_ident)
                    {
                        return self.emit_proto_builtin_call(
                            ta_builtin,
                            member_expr,
                            &call.args,
                            block,
                        );
                    }
                    // SharedArrayBuffer.prototype 方法调用优化（带 receiver guard）。
                    // 必须在 String 之前，以确保 sab.slice 等优先匹配；仅当 obj 是已知 SAB 绑定时才拦截，
                    // 否则回退通用路径，避免劫持 String.prototype.slice / Array 等同名方法（P1 修复）。
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
                        && let Some(sab_builtin) =
                            builtin_from_sharedarraybuffer_proto_method(&prop_ident.sym)
                        && let swc_ast::Expr::Ident(receiver_ident) = member_expr.obj.as_ref()
                        && self.is_sharedarraybuffer_binding(receiver_ident)
                    {
                        return self.emit_proto_builtin_call(
                            sab_builtin,
                            member_expr,
                            &call.args,
                            block,
                        );
                    }
                    // DataView.prototype get/set 方法使用非 Type 12 的专用宿主导入；
                    // 对静态已知 DataView receiver 直连 CallBuiltin，避免通用 call_indirect 调用约定不匹配。
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
                        && let Some(dv_builtin) =
                            builtin_from_dataview_proto_method(&prop_ident.sym)
                        && let swc_ast::Expr::Ident(receiver_ident) = member_expr.obj.as_ref()
                        && self.is_dataview_binding(receiver_ident)
                    {
                        return self.emit_proto_builtin_call(
                            dv_builtin,
                            member_expr,
                            &call.args,
                            block,
                        );
                    }
                    // Map.prototype 方法调用优化（带 receiver guard）。
                    // 仅静态已知 Map 绑定直连 CallBuiltin（new Map() 赋值/声明），
                    // 免去每次调用的通用 Get + NativeCallable dispatch 往返。
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
                        && let Some(map_builtin) = builtin_from_map_proto_method(&prop_ident.sym)
                        && let swc_ast::Expr::Ident(receiver_ident) = member_expr.obj.as_ref()
                        && self.is_map_binding(receiver_ident)
                    {
                        return self.emit_proto_builtin_call(
                            map_builtin,
                            member_expr,
                            &call.args,
                            block,
                        );
                    }
                    // Set.prototype 方法调用优化（带 receiver guard）。
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
                        && let Some(set_builtin) = builtin_from_set_proto_method(&prop_ident.sym)
                        && let swc_ast::Expr::Ident(receiver_ident) = member_expr.obj.as_ref()
                        && self.is_set_binding(receiver_ident)
                    {
                        return self.emit_proto_builtin_call(
                            set_builtin,
                            member_expr,
                            &call.args,
                            block,
                        );
                    }
                    // Array.prototype 方法调用优化（带 receiver guard）。
                    // 仅静态已知 Array receiver 直连（含链式高阶函数中间结果）；
                    // Map/Set 的 forEach/entries 等必须走自身方法。运行时的
                    // 自有覆盖 / 原型改写 / delete 由 pristine 守卫分流。
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
                        && let Some(array_builtin) =
                            builtin_from_array_proto_method(&prop_ident.sym)
                        && self.is_array_producing_expr(member_expr.obj.as_ref())
                    {
                        return self.lower_intrinsic_guarded_proto_call(
                            wjsm_ir::constants::INTRINSIC_FAMILY_ARRAY_PROTO,
                            array_builtin,
                            member_expr,
                            &call.args,
                            block,
                        );
                    }
                    // String.prototype 方法调用优化。pristine 守卫内含字符串
                    // receiver 判定：与 Array 同名的方法（concat/includes/
                    // indexOf/lastIndexOf/slice）只在 receiver 可证明为字符串
                    // 时启用守卫直连，否则保持通用路径避免静态劫持；其余方法
                    // 名对非字符串 receiver（自有同名方法的普通对象）由守卫
                    // 判 false 落入通用属性查找 + 动态调用。
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
                        && let Some(string_builtin) =
                            builtin_from_string_proto_method(&prop_ident.sym)
                        && (self.is_string_producing_expr(member_expr.obj.as_ref())
                            || !matches!(
                                prop_ident.sym.as_ref(),
                                "concat" | "includes" | "indexOf" | "lastIndexOf" | "slice"
                            )
                            || matches!(member_expr.obj.as_ref(), swc_ast::Expr::Ident(receiver_ident)
                                if self.is_maybe_string_binding(receiver_ident)))
                    {
                        return self.lower_intrinsic_guarded_proto_call(
                            wjsm_ir::constants::INTRINSIC_FAMILY_STRING_PROTO,
                            string_builtin,
                            member_expr,
                            &call.args,
                            block,
                        );
                    }

                    // RegExp.prototype 方法调用优化：RegExp 宿主函数使用固定二参调用约定，
                    // 不能通过运行时属性查找后再走通用 call_indirect。
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
                        && let Some(regexp_builtin) =
                            builtin_from_regexp_proto_method(&prop_ident.sym)
                    {
                        let mut call_block = block;
                        this_val = self
                            .lower_call_operand_then_continue(&member_expr.obj, &mut call_block)?;
                        let mut builtin_args = vec![this_val];
                        if Self::call_args_have_spread(&call.args) {
                            let (args_array, end_block) =
                                self.lower_call_args_to_array(&call.args, call_block)?;
                            call_block = end_block;
                            builtin_args
                                .push(self.lower_call_array_element(args_array, 0, call_block));
                        } else if let Some(arg) = call.args.first() {
                            builtin_args.push(
                                self.lower_call_operand_then_continue(&arg.expr, &mut call_block)?,
                            );
                        } else {
                            let undef_const = self.module.add_constant(Constant::Undefined);
                            let undef_val = self.alloc_value();
                            self.current_function.append_instruction(
                                call_block,
                                Instruction::Const {
                                    dest: undef_val,
                                    constant: undef_const,
                                },
                            );
                            builtin_args.push(undef_val);
                        }
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            call_block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: regexp_builtin,
                                args: builtin_args,
                            },
                        );
                        if call_block != block {
                            self.expr_merge_block = Some(call_block);
                        }
                        return Ok(dest);
                    }

                    // 仅优化 Object.prototype.xxx.call(thisArg) 形态：
                    // Object.prototype.toString.call(obj) → CallBuiltin(ObjectProtoToString, obj)
                    // 禁止把任意 .call/.apply/.bind 静态改写为 Function.prototype 方法，
                    // 否则会劫持 dgram.Socket.prototype.bind 等普通对象方法名。
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
                        && prop_ident.sym.as_ref() == "call"
                        && let Some(object_proto_builtin) =
                            self.is_object_proto_method_access(&member_expr.obj)
                    {
                        let mut call_block = block;
                        let this_arg = if Self::call_args_have_spread(&call.args) {
                            let (args_array, end_block) =
                                self.lower_call_args_to_array(&call.args, call_block)?;
                            call_block = end_block;
                            self.lower_call_array_element(args_array, 0, call_block)
                        } else if let Some(first_arg) = call.args.first() {
                            self.lower_call_operand_then_continue(&first_arg.expr, &mut call_block)?
                        } else {
                            let undef_const = self.module.add_constant(Constant::Undefined);
                            let undef_val = self.alloc_value();
                            self.current_function.append_instruction(
                                call_block,
                                Instruction::Const {
                                    dest: undef_val,
                                    constant: undef_const,
                                },
                            );
                            undef_val
                        };
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            call_block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: object_proto_builtin,
                                args: vec![this_arg],
                            },
                        );
                        if call_block != block {
                            self.expr_merge_block = Some(call_block);
                        }
                        return Ok(dest);
                    }

                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop {
                        // Object.prototype 方法调用优化：hasOwnProperty
                        if let Some(obj_proto_builtin) =
                            builtin_from_object_proto_method(&prop_ident.sym)
                        {
                            // obj.method() → obj 是 this
                            return self.emit_proto_builtin_call(
                                obj_proto_builtin,
                                member_expr,
                                &call.args,
                                block,
                            );
                        }

                        if let Some(promise_proto_builtin) =
                            builtin_from_promise_proto_method(&prop_ident.sym)
                        {
                            let mut call_block = block;
                            this_val = self.lower_call_operand_then_continue(
                                &member_expr.obj,
                                &mut call_block,
                            )?;
                            let mut builtin_args = vec![this_val];
                            let required_args: usize = match promise_proto_builtin {
                                Builtin::PromiseThen => 3,
                                Builtin::PromiseCatch | Builtin::PromiseFinally => 2,
                                _ => 1,
                            };
                            if Self::call_args_have_spread(&call.args) {
                                let (args_array, end_block) =
                                    self.lower_call_args_to_array(&call.args, call_block)?;
                                call_block = end_block;
                                for index in 0..required_args.saturating_sub(1) {
                                    let index = u32::try_from(index)
                                        .expect("promise argument index fits u32");
                                    builtin_args.push(
                                        self.lower_call_array_element(
                                            args_array, index, call_block,
                                        ),
                                    );
                                }
                            } else {
                                for arg in &call.args {
                                    builtin_args.push(self.lower_call_operand_then_continue(
                                        &arg.expr,
                                        &mut call_block,
                                    )?);
                                }
                                while builtin_args.len() < required_args {
                                    let undef_const = self.module.add_constant(Constant::Undefined);
                                    let undef_val = self.alloc_value();
                                    self.current_function.append_instruction(
                                        call_block,
                                        Instruction::Const {
                                            dest: undef_val,
                                            constant: undef_const,
                                        },
                                    );
                                    builtin_args.push(undef_val);
                                }
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                call_block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: promise_proto_builtin,
                                    args: builtin_args,
                                },
                            );
                            if call_block != block {
                                self.expr_merge_block = Some(call_block);
                            }
                            return Ok(dest);
                        }

                        if let Some(number_proto_builtin) =
                            builtin_from_number_proto_method(&prop_ident.sym)
                            && self.should_use_number_proto_call_fast_path(
                                prop_ident.sym.as_ref(),
                                member_expr.obj.as_ref(),
                            )
                        {
                            return self.emit_proto_builtin_call(
                                number_proto_builtin,
                                member_expr,
                                &call.args,
                                block,
                            );
                        }

                        if let Some(boolean_proto_builtin) =
                            builtin_from_boolean_proto_method(&prop_ident.sym)
                        {
                            return self.emit_proto_builtin_call(
                                boolean_proto_builtin,
                                member_expr,
                                &call.args,
                                block,
                            );
                        }
                    }

                    // obj.method() → obj 是 this，method 是 callee（未被拦截时）。
                    // obj 可能因捕获绑定读取产生分支/phi，后续取属性必须接在继续块上；
                    // receiver 求值抛出时必须在取属性前中止并传播。
                    let mut member_block = block;
                    this_val =
                        self.lower_call_operand_then_continue(&member_expr.obj, &mut member_block)?;
                    callee_val = self.lower_member_expr_from_object(
                        member_expr,
                        this_val,
                        &mut member_block,
                        false,
                    )?;
                    // 方法查找（getter / Proxy get 陷阱）抛出必须先于调用分叉
                    // 传播，哨兵不得作为 callee 流入 Call（否则误报
                    // "... is not a function" 且丢失原始异常）。
                    member_block = self.lower_value_exception_branch(member_block, callee_val)?;
                    callee_block = member_block;
                } else if let swc_ast::Expr::Ident(ident) = expr.as_ref()
                    && self.eval_scope_record
                    && self.eval_scope_bridge_active()
                    && self.scopes.lookup(&ident.sym).is_err()
                {
                    // eval 代码的自由名 callee：ScopeRecord 可能携带调用方的
                    // with 链，this 绑定按 WithBaseObject（§9.1.1.2.10）由宿主
                    // 解析（无 with 层时恒 undefined）。
                    let env = self.load_eval_scope_env(block);
                    let key = self.append_eval_env_key_const(block, ident.sym.as_ref());
                    this_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::CallBuiltin {
                            dest: Some(this_val),
                            builtin: Builtin::EvalWithBase,
                            args: vec![env, key],
                        },
                    );
                    let this_cont = self.lower_value_exception_branch(block, this_val)?;
                    let mut callee_eval_block = this_cont;
                    callee_val =
                        self.lower_call_operand_then_continue(expr, &mut callee_eval_block)?;
                    callee_block = callee_eval_block;
                } else {
                    // 普通调用 → this = undefined
                    let undef_const = self.module.add_constant(Constant::Undefined);
                    this_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: this_val,
                            constant: undef_const,
                        },
                    );
                    // callee 表达式（如 `(f())()`）抛出时必须在调用前中止并传播。
                    let mut callee_eval_block = block;
                    callee_val =
                        self.lower_call_operand_then_continue(expr, &mut callee_eval_block)?;
                    callee_block = callee_eval_block;
                }
            }
            swc_ast::Callee::Import { .. } => {
                // 动态 import() 调用
                return self.lower_dynamic_import_call(call, block);
            }
            swc_ast::Callee::Super(super_token) => {
                if !self.super_call_allowed {
                    return Err(self.error(
                        super_token.span,
                        "super() is only valid inside derived constructors",
                    ));
                }
                // super() 只在显式派生构造器（含其内层箭头）持有实例原型
                // 绑定；字段初始化器等其余位置按早错误拒绝（对齐 V8 文案）。
                let Some(proto_binding) = self.ctor_super_proto.clone() else {
                    return Err(self.error(super_token.span, "'super' keyword unexpected here"));
                };
                let callee = self.alloc_value();
                self.current_function
                    .append_instruction(block, Instruction::GetSuperConstructor { dest: callee });
                // 原型读取可能经捕获链引入控制流（箭头帧），先解析到延续块；
                // thisArgument 本身须在实参求值之后新建（规范 [[Construct]]
                // 顺序），故此处只读原型值。
                let proto_val = self.emit_read_ctor_super_proto(block, &proto_binding)?;
                let mut call_block = self.resolve_store_block(block);
                let ctor_result = self.alloc_value();
                let this_val;
                if Self::call_args_have_spread(&call.args) {
                    let (args_array, end_block) =
                        self.lower_call_args_to_array(&call.args, call_block)?;
                    call_block = end_block;
                    this_val = self.emit_super_this_argument(call_block, proto_val);
                    self.current_function.append_instruction(
                        call_block,
                        Instruction::CallBuiltin {
                            dest: Some(ctor_result),
                            builtin: Builtin::SuperApply,
                            args: vec![callee, this_val, args_array],
                        },
                    );
                } else {
                    let mut args = Vec::with_capacity(call.args.len());
                    for arg in &call.args {
                        args.push(
                            self.lower_call_operand_then_continue(&arg.expr, &mut call_block)?,
                        );
                    }
                    this_val = self.emit_super_this_argument(call_block, proto_val);
                    self.current_function.append_instruction(
                        call_block,
                        Instruction::SuperCall {
                            dest: Some(ctor_result),
                            callee,
                            this_val,
                            args,
                            forward_args: false,
                        },
                    );
                }
                // 实参异常分叉与 emit_super_call_result_bind 都会引入控制流并
                // 终结入口块；必须把 merge 块经 expr_merge_block 上报，否则
                // 外层（语句级异常检查、后续表达式）的启发式解析穿不过分叉链，
                // 会误写已终结块并覆盖其终结器。
                let (result, merge_block) =
                    self.emit_super_call_result_bind(call_block, ctor_result, this_val)?;
                // InitializeInstanceElements（ES SuperCall 步骤 11）：字段
                // 初始化属于 super() 求值本身——BindThisValue 之后、表达式
                // 返回之前发射，任何位置（语句、赋值右值、if 分支、箭头体内）
                // 都成立。持有初始化上下文的帧（构造器帧及其内层箭头帧）发射；
                // Construct 异常须先分叉传播，不得触达初始化器。
                let continuation = self.emit_super_site_instance_inits(merge_block, result)?;
                self.expr_merge_block = Some(continuation);
                return Ok(result);
            }
        }
        let mut call_block = self.resolve_store_block(callee_block);
        let dest;
        if Self::call_args_have_spread(&call.args) {
            let (args_array, end_block) = self.lower_call_args_to_array(&call.args, call_block)?;
            call_block = end_block;
            dest = self.alloc_value();
            self.current_function.append_instruction(
                call_block,
                Instruction::CallBuiltin {
                    dest: Some(dest),
                    builtin: Builtin::FuncApply,
                    args: vec![callee_val, this_val, args_array],
                },
            );
        } else {
            let mut args = Vec::with_capacity(call.args.len());
            for arg in &call.args {
                args.push(self.lower_call_operand_then_continue(&arg.expr, &mut call_block)?);
            }
            dest = self.alloc_value();
            self.current_function.append_instruction(
                call_block,
                Instruction::Call {
                    dest: Some(dest),
                    callee: callee_val,
                    this_val,
                    args,
                },
            );
        }
        // callee/member receiver 或参数求值引入控制流（如 `new C().m()`、
        // `new RegExp(...)` 的异常分叉、三元、await 等）时，Call 已发射在推进后的
        // 延续块上。必须把该块经 expr_merge_block 上报，否则外层语句会在过时的入口块
        // 上继续，覆盖延续块的终结器并使真正的 Call 落入不可达块。
        if call_block != block {
            self.expr_merge_block = Some(call_block);
        }
        Ok(dest)
    }

    /// 可选调用 `callee?.(args)`：
    /// - 静态宿主 API（`console.log` 等）与普通调用一样走 `CallBuiltin`（属性不存在于对象图，
    ///   只能靠编译期识别；`?.` 在静态已知方法上恒存在，与 V8 优化路径一致）。
    /// - 其它 callee 发射 `OptionalCall`，由 backend 对 null/undefined 短路。
    pub(crate) fn lower_optional_call_expr(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if let swc_ast::Callee::Expr(expr) = &call.callee
            && let swc_ast::Expr::Member(member_expr) = expr.as_ref()
            && let swc_ast::Expr::Ident(obj_ident) = member_expr.obj.as_ref()
            && let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop
            && let Some(builtin) = builtin_from_static_member(&obj_ident.sym, &prop_ident.sym)
            && !self.global_intrinsic_shadowed(&obj_ident.sym)
            && self
                .with_scopes_for_ident(obj_ident.sym.as_ref())
                .is_empty()
        {
            // pristine 时静态方法恒存在，可选链不短路，等于直接 CallBuiltin；
            // 被改写 / 删除后慢路径按可选调用对 nullish callee 短路。
            return self.lower_intrinsic_guarded_call(
                call,
                block,
                builtin,
                IntrinsicCallSite::StaticMember { optional: true },
            );
        }

        let callee_val: ValueId;
        let this_val: ValueId;
        let callee_block: BasicBlockId;

        // 成员形态的 callee（含 `a?.b()` / `a?.b.c()` 的 OptChain 包装）：
        // receiver 求值一次并复用为 this（EvaluateCall 的 thisValue 取自
        // Reference base，`f().m?.()` 不得二次求值）；`?.` 短路点按包装节点的
        // optional 发 OptionalGetProp，短路产出 undefined 后 OptionalCall 再次
        // 短路，与规范链式短路一致。
        let callee_member = match &call.callee {
            swc_ast::Callee::Expr(expr) => match expr.as_ref() {
                swc_ast::Expr::Member(member_expr) => Some((member_expr, false)),
                swc_ast::Expr::OptChain(oc) => match oc.base.as_ref() {
                    swc_ast::OptChainBase::Member(member_expr) => Some((member_expr, oc.optional)),
                    swc_ast::OptChainBase::Call(_) => None,
                },
                _ => None,
            },
            _ => None,
        };

        match &call.callee {
            swc_ast::Callee::Expr(expr) => {
                if let Some((member_expr, member_optional)) = callee_member {
                    // 方法查找（getter）抛出必须先于调用分叉传播，哨兵不得作为
                    // callee 流入 OptionalCall。
                    let mut member_block = block;
                    this_val =
                        self.lower_call_operand_then_continue(&member_expr.obj, &mut member_block)?;
                    callee_val = self.lower_member_expr_from_object(
                        member_expr,
                        this_val,
                        &mut member_block,
                        member_optional,
                    )?;
                    member_block = self.lower_value_exception_branch(member_block, callee_val)?;
                    callee_block = member_block;
                } else if let swc_ast::Expr::Ident(ident) = expr.as_ref()
                    && !self.with_scopes_for_ident(ident.sym.as_ref()).is_empty()
                {
                    // 裸标识符可选调用穿越 with 作用域：callee/this 动态分派，
                    // 命中 with 对象时 this 绑定为该对象（§9.1.1.2.10）。
                    let crossed = self.with_scopes_for_ident(ident.sym.as_ref());
                    let (callee, this, post) =
                        self.lower_with_callee_resolution(ident, &crossed, block)?;
                    callee_val = callee;
                    this_val = this;
                    callee_block = post;
                } else {
                    this_val = self.alloc_value();
                    let undef = self.module.add_constant(Constant::Undefined);
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: this_val,
                            constant: undef,
                        },
                    );
                    // callee 表达式（`o?.m` 等可选链）抛出必须在调用前分叉传播。
                    let mut callee_eval_block = block;
                    callee_val =
                        self.lower_call_operand_then_continue(expr, &mut callee_eval_block)?;
                    callee_block = callee_eval_block;
                }
            }
            other => {
                let _ = other;
                return self.lower_call_expr(call, block);
            }
        }

        let call_block = self.resolve_store_block(callee_block);
        if !call.args.is_empty() {
            // 有实参：短路点必须在 ArgumentListEvaluation 之前（§13.3.9.1），
            // nullish callee 不得触发实参求值的副作用。
            return self.lower_optional_call_short_circuit(
                callee_val,
                this_val,
                &call.args,
                call_block,
            );
        }

        // 无实参：不存在实参求值顺序问题，OptionalCall 自身完成 nullish 短路。
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            call_block,
            Instruction::OptionalCall {
                dest,
                callee: callee_val,
                this_val,
                args: Vec::new(),
            },
        );
        if call_block != block {
            self.expr_merge_block = Some(call_block);
        }
        Ok(dest)
    }

    pub(crate) fn lower_direct_eval_call(
        &mut self,
        call: &swc_ast::CallExpr,
        block: BasicBlockId,
    ) -> Result<(ValueId, BasicBlockId), LoweringError> {
        let mut eval_block = block;
        self.current_function.mark_has_eval();

        // 1. Lower the code argument（实参抛出必须在构建 ScopeRecord 前中止并传播）
        let code_val = if let Some(first_arg) = call.args.first() {
            self.lower_call_operand_then_continue(&first_arg.expr, &mut eval_block)?
        } else {
            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                eval_block,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );
            undef_val
        };

        // 2. Get all lexically visible bindings (including TDZ)
        // 脚本全局绑定不进快照：宿主 eval 边界解析（EvalGet/Set/HasBinding）
        // 穿透 ScopeRecord 后按全局环境记录（声明式 → 对象）命中，快照副本
        // 反而会遮蔽真值并造成写入分叉。
        let all_bindings: Vec<_> = self
            .scopes
            .visible_bindings_all()
            .into_iter()
            .filter(|(_, name, _, _)| !matches!(name.as_str(), "undefined" | "NaN" | "Infinity"))
            .filter(|(scope_id, name, _, _)| {
                !(*scope_id == 0 && self.script_global_names.contains_key(name))
            })
            .collect();

        // 3. Create ScopeRecord
        let capacity = self.const_val_i64(eval_block, all_bindings.len() as i64);
        let scope_record = self.alloc_value();
        self.current_function.append_instruction(
            eval_block,
            Instruction::CallBuiltin {
                dest: Some(scope_record),
                builtin: Builtin::ScopeRecordCreate,
                args: vec![capacity],
            },
        );

        // 将 scope_record 写入 $eval_env，供 eval 模块读取。
        self.current_function.append_instruction(
            eval_block,
            Instruction::StoreVar {
                name: EVAL_SCOPE_ENV_PARAM.to_string(),
                value: scope_record,
            },
        );

        // 4. Add each binding to the ScopeRecord
        for (scope_id, name, kind, is_initialised) in &all_bindings {
            let name_const = self.module.add_constant(Constant::String(name.clone()));
            let name_val = self.alloc_value();
            self.current_function.append_instruction(
                eval_block,
                Instruction::Const {
                    dest: name_val,
                    constant: name_const,
                },
            );

            let binding = CapturedBinding::new(name.clone(), *scope_id);
            let value_from_env = !self.binding_belongs_to_current_function(&binding)
                || self.is_shared_binding(&binding);
            let value = if value_from_env {
                // 共享/捕获绑定读取会分叉（shared env 探测 branch + phi），
                // 必须消化续接并推进插入点，否则后续指令覆盖分支终结器、
                // phi 结果悬空（invalid IR）。
                let value = self.load_captured_binding(eval_block, &binding)?;
                self.resolve_expr_continuations(&mut eval_block);
                value
            } else {
                let value = self.alloc_value();
                self.current_function.append_instruction(
                    eval_block,
                    Instruction::LoadVar {
                        dest: value,
                        name: binding.var_ir_name(),
                    },
                );
                value
            };

            // env 取值路径的 TDZ 状态由宿主按未初始化哨兵动态判定（跨函数前向
            // 引用的绑定在本 eval 站点降级时静态标志可能已过期）；本地槽路径
            // 与直线执行一致，静态标志即运行时状态。
            let is_tdz_flag = if value_from_env {
                0
            } else if *is_initialised {
                0
            } else {
                1
            };
            let is_tdz = self.const_val_i64(eval_block, is_tdz_flag);
            // 不可变形态编码：1 = const（S=true，写恒 TypeError）；2 = 具名
            // 函数表达式自身名字（S=false，非严格写静默忽略）；0 = 可变。
            let is_const = self.const_val_i64(
                eval_block,
                if self.scopes.is_fn_expr_name(*scope_id, name) {
                    2
                } else if matches!(kind, VarKind::Const) {
                    1
                } else {
                    0
                },
            );

            self.current_function.append_instruction(
                eval_block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ScopeRecordAddBinding,
                    args: vec![scope_record, name_val, value, is_tdz, is_const],
                },
            );
        }

        // 4b. 包围 eval 站点的 with 链（§9.1.1.2 对象环境记录）：按解析序
        // （由内到外）追加。inner_names 为声明于该层内侧的可见绑定名——
        // 解析这些名字时静态绑定先于该层对象命中，宿主 EvalGet/Set/HasBinding
        // 据此在静态绑定与 with 对象之间正确插层。
        for with_scope_id in self.enclosing_with_scopes() {
            let object = self.load_with_object(&mut eval_block, with_scope_id)?;
            let inner_names = all_bindings
                .iter()
                .filter(|(scope_id, ..)| self.scopes.is_strict_ancestor(with_scope_id, *scope_id))
                .map(|(_, name, ..)| name.as_str())
                .collect::<Vec<_>>()
                .join("\0");
            let names_const = self.module.add_constant(Constant::String(inner_names));
            let names_val = self.alloc_value();
            self.current_function.append_instruction(
                eval_block,
                Instruction::Const {
                    dest: names_val,
                    constant: names_const,
                },
            );
            self.current_function.append_instruction(
                eval_block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ScopeRecordAddWithLayer,
                    args: vec![scope_record, object, names_val],
                },
            );
        }

        // 5. Set meta: strict mode (key=0)
        let strict_key = self.const_val_i64(eval_block, 0);
        let strict_val = self.const_val_i64(eval_block, if self.strict_mode { 1 } else { 0 });
        self.current_function.append_instruction(
            eval_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ScopeRecordSetMeta,
                args: vec![scope_record, strict_key, strict_val],
            },
        );

        // 6. Set meta: has_arguments (key=1)
        let args_key = self.const_val_i64(eval_block, 1);
        let args_val = self.const_val_i64(
            eval_block,
            if self.eval_caller_has_arguments { 1 } else { 0 },
        );
        self.current_function.append_instruction(
            eval_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ScopeRecordSetMeta,
                args: vec![scope_record, args_key, args_val],
            },
        );

        // 7. Set meta: super base (key=2). 非方法上下文会得到 undefined。
        let super_key = self.const_val_i64(eval_block, 2);
        let super_base = self.alloc_value();
        self.current_function
            .append_instruction(eval_block, Instruction::GetSuperBase { dest: super_base });
        let super_name = self
            .module
            .add_constant(Constant::String("__wjsm_super_base".to_string()));
        let super_name_val = self.alloc_value();
        self.current_function.append_instruction(
            eval_block,
            Instruction::Const {
                dest: super_name_val,
                constant: super_name,
            },
        );
        let super_false = self.const_val_i64(eval_block, 0);
        self.current_function.append_instruction(
            eval_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ScopeRecordAddBinding,
                args: vec![
                    scope_record,
                    super_name_val,
                    super_base,
                    super_false,
                    super_false,
                ],
            },
        );
        self.current_function.append_instruction(
            eval_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ScopeRecordSetMeta,
                args: vec![scope_record, super_key, super_base],
            },
        );
        // 7b. new.target (meta key=3). 箭头函数从词法环境捕获，普通函数读取当前调用上下文。
        let nt_key = self.const_val_i64(eval_block, 3);
        let new_target = if self.is_arrow {
            let binding = CapturedBinding::lexical_new_target();
            self.record_capture(binding.clone());
            let env_val = self.load_env_object(eval_block);
            let key_val = self.append_env_key_const(eval_block, &binding);
            let new_target = self.alloc_value();
            self.current_function.append_instruction(
                eval_block,
                Instruction::GetProp {
                    dest: new_target,
                    object: env_val,
                    key: key_val,
                },
            );
            new_target
        } else {
            let new_target = self.alloc_value();
            let dummy_const = self.module.add_constant(Constant::Undefined);
            let dummy_val = self.alloc_value();
            self.current_function.append_instruction(
                eval_block,
                Instruction::Const {
                    dest: dummy_val,
                    constant: dummy_const,
                },
            );
            self.current_function.append_instruction(
                eval_block,
                Instruction::CallBuiltin {
                    dest: Some(new_target),
                    builtin: Builtin::NewTarget,
                    args: vec![dummy_val],
                },
            );
            new_target
        };
        self.current_function.append_instruction(
            eval_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ScopeRecordSetMeta,
                args: vec![scope_record, nt_key, new_target],
            },
        );
        // new.target for eval body: runtime reads scope meta first, then runtime global fallback.

        // 8. Call Eval(code, scope_record)
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            eval_block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::Eval,
                args: vec![code_val, scope_record],
            },
        );

        // 8. Exception check
        let is_exc = self.alloc_value();
        self.current_function.append_instruction(
            eval_block,
            Instruction::IsException {
                dest: is_exc,
                value: dest,
            },
        );
        let mut continue_block = self.current_function.new_block();
        let exc_block = self.current_function.new_block();
        self.current_function.set_terminator(
            eval_block,
            Terminator::Branch {
                condition: is_exc,
                true_block: exc_block,
                false_block: continue_block,
            },
        );

        // 9. Exception path
        let thrown_val = self.alloc_value();
        self.current_function.append_instruction(
            exc_block,
            Instruction::CallBuiltin {
                dest: Some(thrown_val),
                builtin: Builtin::ExceptionValue,
                args: vec![dest],
            },
        );
        self.emit_throw_value(exc_block, thrown_val)?;

        // 10. Writeback: read post-eval values for visible bindings (incl. TDZ let/const after assign)
        for (scope_id, name, _, is_initialised) in &all_bindings {
            if !*is_initialised {
                continue;
            }
            let binding = CapturedBinding::new(name.clone(), *scope_id);

            let name_const = self.module.add_constant(Constant::String(name.clone()));
            let name_val = self.alloc_value();
            self.current_function.append_instruction(
                continue_block,
                Instruction::Const {
                    dest: name_val,
                    constant: name_const,
                },
            );

            // 回写必须平面读取自有绑定：经 EvalGetBinding 一则会被 with 层
            // 拦截、把 with 对象属性错误回写进调用方静态绑定；二则静态标志
            // 为已初始化的绑定仍可能动态处于 TDZ（如派生构造器 super() 前的
            // $this 哨兵），EvalGetBinding 会抛 ReferenceError 且异常编码会
            // 被直接写入槽位——平面读取对 TDZ 返回哨兵，回写哨兵即保持原槽
            // 的 TDZ 状态。
            let value = self.alloc_value();
            self.current_function.append_instruction(
                continue_block,
                Instruction::CallBuiltin {
                    dest: Some(value),
                    builtin: Builtin::ScopeRecordGetBinding,
                    args: vec![scope_record, name_val],
                },
            );

            if self.binding_belongs_to_current_function(&binding) {
                if self.is_shared_binding(&binding) {
                    // Shared env: write back via SetProp on the shared env
                    let env_val = self
                        .shared_env_value()
                        .expect("shared binding must have materialized env");
                    let key_val = self.append_env_key_const(continue_block, &binding);
                    self.emit_set_prop(continue_block, env_val, key_val, value);
                } else {
                    // Direct local var
                    self.current_function.append_instruction(
                        continue_block,
                        Instruction::StoreVar {
                            name: binding.var_ir_name(),
                            value,
                        },
                    );
                }
            } else {
                // 外层 binding：写入声明 frame，禁止在当前 env shadow。
                self.record_capture(binding.clone());
                let start_env = self.load_env_object(continue_block);
                let (owner_block, owner_env) =
                    self.resolve_env_binding_owner(continue_block, start_env, &binding);
                continue_block = owner_block;
                let key_val = self.append_env_key_const(continue_block, &binding);
                self.emit_set_prop(continue_block, owner_env, key_val, value);
            }
        }

        // 11. Destroy ScopeRecord
        self.current_function.append_instruction(
            continue_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ScopeRecordDestroy,
                args: vec![scope_record],
            },
        );

        let merge_block = self.current_function.new_block();
        self.current_function.set_terminator(
            continue_block,
            Terminator::Jump {
                target: merge_block,
            },
        );

        Ok((dest, merge_block))
    }
}
