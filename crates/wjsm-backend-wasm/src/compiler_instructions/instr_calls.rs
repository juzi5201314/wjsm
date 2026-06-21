use super::*;

impl Compiler {
    pub(crate) fn compile_call_with_new_target(
        &mut self,
        dest: &Option<ValueId>,
        callee: ValueId,
        this_val: ValueId,
        args: &[ValueId],
        new_target: Option<ValueId>,
    ) -> Result<()> {
        // Type 12 签名: (i64 env_obj, i64 this_val, i32 args_base, i32 args_count) -> i64。
        // new.target 是调用上下文，普通调用压入 undefined，构造调用压入构造目标。
        let saved_new_target = self.string_concat_scratch_idx;
        let result_scratch = self.call_env_obj_scratch();

        match new_target {
            Some(value) => self.emit(WasmInstruction::LocalGet(self.local_idx(value.0))),
            None => self.emit(WasmInstruction::I64Const(value::encode_undefined())),
        }
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::NewTargetSet],
        ));
        self.emit(WasmInstruction::LocalSet(saved_new_target));

        // Step 1: 保存 shadow_sp 到 scratch local
        self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
        self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));

        // Step 1b: 影子栈边界检查
        self.emit_shadow_stack_overflow_check((args.len() * 8) as i32);

        // Step 2: 将所有参数写入影子栈
        for arg in args {
            self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
            self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
            self.emit(WasmInstruction::I64Store(MemArg {
                offset: 0,
                align: 3,
                memory_index: 0,
            }));
            self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
            self.emit(WasmInstruction::I32Const(8));
            self.emit(WasmInstruction::I32Add);
            self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
        }

        // Step 3: native callable 由宿主运行时执行，普通 JS 函数继续走函数表。
        let call_func_idx_scratch = self.call_func_idx_scratch();
        let call_env_obj_scratch = self.call_env_obj_scratch();

        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0x1F));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_NATIVE_CALLABLE as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::LocalGet(self.local_idx(this_val.0)));
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::I32Const(args.len() as i32));
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::NativeCall],
        ));
        self.emit(WasmInstruction::Else);

        // TAG_PROXY 检测: 代理调用走 ProxyApply/ProxyConstruct 宿主函数
        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0x1F));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_PROXY as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
        // Proxy 调用: 通过 ProxyApply 或 ProxyConstruct 宿主函数派发
        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::LocalGet(self.local_idx(this_val.0)));
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::I32Const(args.len() as i32));
        if new_target.is_some() {
            self.emit(WasmInstruction::Call(
                self.special_host_import_indices[&SpecialHostImport::ProxyConstruct],
            ));
        } else {
            self.emit(WasmInstruction::Call(
                self.special_host_import_indices[&SpecialHostImport::ProxyApply],
            ));
        }
        self.emit(WasmInstruction::Else);

        // 运行时解析 callee → (func_idx, env_obj)。callee 可能是 TAG_FUNCTION 或 TAG_CLOSURE。
        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0x1F));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(0xA)); // TAG_CLOSURE
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Empty));
        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::ClosureGetFunc],
        ));
        self.emit(WasmInstruction::LocalSet(call_func_idx_scratch));
        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::ClosureGetEnv],
        ));
        self.emit(WasmInstruction::LocalSet(call_env_obj_scratch));
        self.emit(WasmInstruction::Else);
        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::LocalSet(call_func_idx_scratch));
        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        self.emit(WasmInstruction::LocalSet(call_env_obj_scratch));
        self.emit(WasmInstruction::End);

        self.emit(WasmInstruction::LocalGet(call_env_obj_scratch));
        self.emit(WasmInstruction::LocalGet(self.local_idx(this_val.0)));
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::I32Const(args.len() as i32));
        self.emit(WasmInstruction::LocalGet(call_func_idx_scratch));
        self.emit(WasmInstruction::CallIndirect {
            type_index: 12,
            table_index: 0,
        });
        self.emit(WasmInstruction::End); // close proxy if/else
        self.emit(WasmInstruction::End); // close native callable if/else

        self.emit(WasmInstruction::LocalSet(result_scratch));

        // Step 4: 恢复 new.target 和 shadow_sp
        self.emit(WasmInstruction::LocalGet(saved_new_target));
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::NewTargetSet],
        ));
        self.emit(WasmInstruction::Drop);
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));

        // Step 5: 处理返回值
        self.emit(WasmInstruction::LocalGet(result_scratch));
        if let Some(d) = dest {
            self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
        } else {
            self.emit(WasmInstruction::Drop);
        }

        Ok(())
    }

    pub(crate) fn compile_super_call(
        &mut self,
        dest: &Option<ValueId>,
        callee: ValueId,
        this_val: ValueId,
        args: &[ValueId],
        forward_args: bool,
    ) -> Result<()> {
        let saved_new_target = self.string_concat_scratch_idx;
        let result_scratch = self.call_env_obj_scratch();
        let call_func_idx_scratch = self.call_func_idx_scratch();
        let call_env_obj_scratch = self.call_env_obj_scratch();

        self.emit(WasmInstruction::I64Const(0));
        self.emit(WasmInstruction::Call(
            self.builtin_func_indices[&Builtin::NewTarget],
        ));
        self.emit(WasmInstruction::LocalSet(saved_new_target));

        if forward_args {
            self.emit(WasmInstruction::LocalGet(2));
            self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));
        } else {
            self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
            self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));
            self.emit_shadow_stack_overflow_check((args.len() * 8) as i32);
            for arg in args {
                self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
                self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                self.emit(WasmInstruction::I64Store(MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                }));
                self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
                self.emit(WasmInstruction::I32Const(8));
                self.emit(WasmInstruction::I32Add);
                self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
            }
        }

        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0x1F));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_NATIVE_CALLABLE as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::LocalGet(self.local_idx(this_val.0)));
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        if forward_args {
            self.emit(WasmInstruction::LocalGet(3));
        } else {
            self.emit(WasmInstruction::I32Const(args.len() as i32));
        }
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::NativeCall],
        ));
        self.emit(WasmInstruction::Else);

        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0x1F));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_PROXY as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::LocalGet(self.local_idx(this_val.0)));
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        if forward_args {
            self.emit(WasmInstruction::LocalGet(3));
        } else {
            self.emit(WasmInstruction::I32Const(args.len() as i32));
        }
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::ProxyConstruct],
        ));
        self.emit(WasmInstruction::Else);

        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0x1F));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_CLOSURE as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Empty));
        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::ClosureGetFunc],
        ));
        self.emit(WasmInstruction::LocalSet(call_func_idx_scratch));
        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::ClosureGetEnv],
        ));
        self.emit(WasmInstruction::LocalSet(call_env_obj_scratch));
        self.emit(WasmInstruction::Else);
        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::LocalSet(call_func_idx_scratch));
        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        self.emit(WasmInstruction::LocalSet(call_env_obj_scratch));
        self.emit(WasmInstruction::End);

        self.emit(WasmInstruction::LocalGet(call_env_obj_scratch));
        self.emit(WasmInstruction::LocalGet(self.local_idx(this_val.0)));
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        if forward_args {
            self.emit(WasmInstruction::LocalGet(3));
        } else {
            self.emit(WasmInstruction::I32Const(args.len() as i32));
        }
        self.emit(WasmInstruction::LocalGet(call_func_idx_scratch));
        self.emit(WasmInstruction::CallIndirect {
            type_index: 12,
            table_index: 0,
        });
        self.emit(WasmInstruction::End);
        self.emit(WasmInstruction::End);
        self.emit(WasmInstruction::LocalSet(result_scratch));

        self.emit(WasmInstruction::LocalGet(saved_new_target));
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::NewTargetSet],
        ));
        self.emit(WasmInstruction::Drop);
        if !forward_args {
            self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
            self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
        }

        self.emit(WasmInstruction::LocalGet(result_scratch));
        if let Some(d) = dest {
            self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
        } else {
            self.emit(WasmInstruction::Drop);
        }
        Ok(())
    }

    pub(crate) fn compile_compare(
        &mut self,
        dest: ValueId,
        op: CompareOp,
        lhs: ValueId,
        rhs: ValueId,
    ) -> Result<()> {
        // For Phase 3: implement strict equality and numeric comparisons.
        // All values are i64 NaN-boxed.
        //
        // For strict equality: check if both are f64, then compare as f64.
        // For numeric comparisons: reinterpret as f64 and compare.
        //
        // The result is a NaN-boxed bool (BOX_BASE | TAG_BOOL << 32 | 0 or 1).

        let box_base = value::BOX_BASE as i64;
        match op {
            CompareOp::StrictEq | CompareOp::StrictNotEq => {
                // StrictEq: 类型相同且值相同。
                // 对于两个 plain f64（非 NaN-boxed），使用 f64.eq：
                //   - 0 === -0 → true ✓
                //   - NaN === NaN → false ✓
                // 对于两个 NaN-boxed 值，使用 i64 eq 比较原始位：
                //   - null === null → true ✓
                //   - null === undefined → false（tag 不同）✓
                //   - bool/string/handle 同类型同值 → true ✓
                // 混合类型（一个 f64 一个 NaN-boxed）→ false ✓

                // 检查 lhs 是否为 plain f64：(lhs & BOX_BASE) != BOX_BASE
                self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                self.emit(WasmInstruction::I64Const(box_base));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(box_base));
                self.emit(WasmInstruction::I64Ne); // 1 if lhs is plain f64

                // 检查 rhs 是否为 plain f64
                self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                self.emit(WasmInstruction::I64Const(box_base));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(box_base));
                self.emit(WasmInstruction::I64Ne); // 1 if rhs is plain f64

                // both_f64 = lhs_is_f64 && rhs_is_f64
                self.emit(WasmInstruction::I32And);

                self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
                // 两者都是 plain f64：使用 f64.eq
                self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                self.emit(WasmInstruction::F64ReinterpretI64);
                self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                self.emit(WasmInstruction::F64ReinterpretI64);
                self.emit(WasmInstruction::F64Eq);
                self.emit(WasmInstruction::Else);
                // 至少一个是 NaN-boxed：使用 i64 位比较
                self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                self.emit(WasmInstruction::I64Eq);
                self.emit(WasmInstruction::End);

                if matches!(op, CompareOp::StrictNotEq) {
                    self.emit(WasmInstruction::I32Const(1));
                    self.emit(WasmInstruction::I32Xor);
                }

                // 将 i32 bool 转为 NaN-boxed bool
                self.emit(WasmInstruction::I64ExtendI32U);
                let tag_bool = (value::TAG_BOOL << 32) as i64;
                self.emit(WasmInstruction::I64Const(box_base | tag_bool));
                self.emit(WasmInstruction::I64Or);
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
            }
        }

        Ok(())
    }

    /// 编译 Array.prototype 方法调用（Type 12 导入函数）。
    /// 将 IR 层的 CallBuiltin 转换为对 Type 12 宿主函数的调用。
    /// 通过影子栈传递参数，参数布局：
    ///   env_obj=undefined, this_val=args[0], shadow_args=args[1..]
    /// 特例：ArrayIsArray 的 this_val=undefined, shadow_args=args
    pub(crate) fn compile_proto_method_call(
        &mut self,
        dest: Option<ValueId>,
        builtin: &Builtin,
        args: &[ValueId],
    ) -> Result<()> {
        let import_idx = if matches!(builtin, Builtin::ArrayFlat) {
            self.special_host_import_indices
                .get(&SpecialHostImport::ArrayProtoFlat)
                .copied()
                .with_context(|| format!("no WASM func index for builtin {builtin}"))?
        } else {
            self.builtin_func_indices
                .get(builtin)
                .copied()
                .with_context(|| format!("no WASM func index for builtin {builtin}"))?
        };
        // 确定 this_val 和影子栈参数
        // ArrayIsArray: this_val=undefined, 所有 args 走影子栈
        // FuncCall/FuncBind: env_obj=func, this_val=args[1], shadow_args=args[2..]
        // 其他方法: this_val=args[0], args[1..] 走影子栈
        let (env_obj_val, this_val_idx, shadow_args) =
            if matches!(builtin, Builtin::FuncCall | Builtin::FuncBind) {
                // args = [func, this_val, ...restArgs]
                let func: ValueId = args.first().copied().unwrap_or(ValueId(0));
                let this: Option<ValueId> = args.get(1).copied();
                let shadow_slice: &[ValueId] = if args.len() > 2 { &args[2..] } else { &[] };
                (Some(func), this, shadow_slice)
            } else if matches!(
                builtin,
                Builtin::ArrayIsArray
                    | Builtin::ArrayFrom
                    | Builtin::StringFromCharCode
                    | Builtin::StringFromCodePoint
                    | Builtin::MathMax
                    | Builtin::MathMin
                    | Builtin::MathHypot
                    | Builtin::DateConstructor
            ) {
                (None, None, args)
            } else {
                let this = args
                    .first()
                    .with_context(|| format!("{builtin} expects at least 1 argument (this_val)"))?;
                (None, Some(*this), &args[1..])
            };
        // 保存 shadow_sp 基址
        self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
        self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));
        // 影子栈边界检查
        self.emit_shadow_stack_overflow_check((shadow_args.len() * 8) as i32);

        // 将 shadow_args 写入影子栈
        for arg in shadow_args {
            self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
            self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
            self.emit(WasmInstruction::I64Store(MemArg {
                offset: 0,
                align: 3,
                memory_index: 0,
            }));
            self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
            self.emit(WasmInstruction::I32Const(8));
            self.emit(WasmInstruction::I32Add);
            self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
        }
        // 推入 Type 12 调用参数: env_obj, this_val, args_base, args_count
        // env_obj
        if let Some(val) = env_obj_val {
            self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
        } else {
            self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        }
        // this_val
        if let Some(val) = this_val_idx {
            self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
        } else {
            self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        }
        // args_base
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        // args_count
        self.emit(WasmInstruction::I32Const(shadow_args.len() as i32));
        // 调用 Type 12 宿主函数
        self.emit(WasmInstruction::Call(import_idx));
        // 恢复 shadow_sp
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
        // 处理返回值
        if let Some(d) = dest {
            self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
        } else {
            self.emit(WasmInstruction::Drop);
        }
        Ok(())
    }

    pub(crate) fn compile_string_concat_va(
        &mut self,
        dest: &ValueId,
        parts: &[ValueId],
    ) -> Result<()> {
        // 保存 shadow_sp 基址
        self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
        self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));
        // 影子栈边界检查
        self.emit_shadow_stack_overflow_check((parts.len() * 8) as i32);
        // 将 parts 写入影子栈
        for part in parts {
            self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
            self.emit(WasmInstruction::LocalGet(self.local_idx(part.0)));
            self.emit(WasmInstruction::I64Store(MemArg {
                offset: 0,
                align: 3,
                memory_index: 0,
            }));
            self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
            self.emit(WasmInstruction::I32Const(8));
            self.emit(WasmInstruction::I32Add);
            self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
        }
        // 推入 string_concat_va 参数: args_base, args_count
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::I32Const(parts.len() as i32));
        // 调用 import 17: string_concat_va
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::StringConcatVa],
        ));
        // 先保存返回值
        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
        // 恢复 shadow_sp
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
        Ok(())
    }

    /// 编译可选链属性/索引访问：检查 object 是否为 null/undefined，是则返回 undefined
    pub(crate) fn compile_optional_get(
        &mut self,
        dest: &ValueId,
        object: &ValueId,
        is_prop: bool,
        key: Option<&ValueId>,
        _is_call: bool,
    ) -> Result<()> {
        // 提取 tag: (object >> 32) & 0xF
        self.emit(WasmInstruction::LocalGet(self.local_idx(object.0)));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0xF));
        self.emit(WasmInstruction::I64And);

        // 检查是否为 TAG_NULL (0x3) 或 TAG_UNDEFINED (0x2)
        // 先保存 tag 值
        self.emit(WasmInstruction::LocalTee(self.string_concat_scratch_idx));
        self.emit(WasmInstruction::I64Const(value::TAG_NULL as i64));
        self.emit(WasmInstruction::I64Eq);

        self.emit(WasmInstruction::LocalGet(self.string_concat_scratch_idx));
        self.emit(WasmInstruction::I64Const(value::TAG_UNDEFINED as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::I64Or);

        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
        // null/undefined → 返回 encode_undefined()
        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        self.emit(WasmInstruction::Else);
        // 正常路径
        let Some(k) = key else {
            bail!("OptionalGet requires a key");
        };
        if is_prop {
            self.emit(WasmInstruction::LocalGet(self.local_idx(object.0)));
            self.emit(WasmInstruction::LocalGet(self.local_idx(k.0)));
            self.emit(WasmInstruction::Call(
                self.special_host_import_indices[&SpecialHostImport::SymbolPropertyKey],
            ));
            self.emit(WasmInstruction::Call(self.obj_get_func_idx));
        } else {
            // OptionalGetElem：按 key 类型分派（数字→元素，字符串→命名属性）。
            self.emit_computed_get(*object, *k);
        }
        self.emit(WasmInstruction::End);
        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
        Ok(())
    }

    /// 编译可选链调用：callee 为 null/undefined 时返回 undefined，否则正常 call_indirect
    pub(crate) fn compile_optional_call(
        &mut self,
        dest: &ValueId,
        callee: &ValueId,
        this_val: &ValueId,
        args: &[ValueId],
    ) -> Result<()> {
        // 检查 callee 是否为 null/undefined
        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0xF));
        self.emit(WasmInstruction::I64And);

        self.emit(WasmInstruction::LocalTee(self.string_concat_scratch_idx));
        self.emit(WasmInstruction::I64Const(value::TAG_NULL as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::LocalGet(self.string_concat_scratch_idx));
        self.emit(WasmInstruction::I64Const(value::TAG_UNDEFINED as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::I64Or);

        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        self.emit(WasmInstruction::Else);

        // 正常 Call 路径（内联 compile_call 逻辑）
        self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
        self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));

        self.emit_shadow_stack_overflow_check((args.len() * 8) as i32);

        for arg in args.iter() {
            self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
            self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
            self.emit(WasmInstruction::I64Store(MemArg {
                offset: 0,
                align: 3,
                memory_index: 0,
            }));
            self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
            self.emit(WasmInstruction::I32Const(8));
            self.emit(WasmInstruction::I32Add);
            self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
        }

        let call_func_idx_scratch = self.call_func_idx_scratch();
        let call_env_obj_scratch = self.call_env_obj_scratch();

        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0xF));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(0xA));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Empty));
        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::ClosureGetFunc],
        ));
        self.emit(WasmInstruction::LocalSet(call_func_idx_scratch));
        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::ClosureGetEnv],
        ));
        self.emit(WasmInstruction::LocalSet(call_env_obj_scratch));
        self.emit(WasmInstruction::Else);
        self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::LocalSet(call_func_idx_scratch));
        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        self.emit(WasmInstruction::LocalSet(call_env_obj_scratch));
        self.emit(WasmInstruction::End);

        self.emit(WasmInstruction::LocalGet(call_env_obj_scratch));
        self.emit(WasmInstruction::LocalGet(self.local_idx(this_val.0)));
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::I32Const(args.len() as i32));
        self.emit(WasmInstruction::LocalGet(call_func_idx_scratch));
        self.emit(WasmInstruction::CallIndirect {
            type_index: 12,
            table_index: 0,
        });

        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));

        self.emit(WasmInstruction::End);
        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
        Ok(())
    }

}
