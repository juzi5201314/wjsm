use super::*;

impl Compiler {
    pub(crate) fn compile_instruction(
        &mut self,
        module: &IrModule,
        instruction: &Instruction,
    ) -> Result<bool> {
        match instruction {
            Instruction::Const { dest, constant } => {
                let constant = module
                    .constants()
                    .get(constant.0 as usize)
                    .with_context(|| format!("missing constant {constant}"))?;
                // BigInt 常量：嵌入字符串到 data segment，运行时调用 bigint_from_literal
                if let Constant::BigInt(s) = constant {
                    let ptr = self.intern_data_string(s);
                    let len = (s.len() + 1) as i32; // 包含 nul terminator
                    self.emit(WasmInstruction::I32Const(ptr as i32));
                    self.emit(WasmInstruction::I32Const(len));
                    let func_idx = self
                        .builtin_func_indices
                        .get(&Builtin::BigIntFromLiteral)
                        .copied()
                        .expect("BigIntFromLiteral import must be registered");
                    self.emit(WasmInstruction::Call(func_idx));
                    self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                } else if let Constant::RegExp { pattern, flags } = constant {
                    // RegExp 常量：嵌入 pattern 和 flags 到 data segment，运行时调用 regex_create
                    let pat_ptr = self.intern_data_string(pattern);
                    let pat_len = (pattern.len() + 1) as i32; // 包含 nul terminator
                    let flags_ptr = self.intern_data_string(flags);
                    let flags_len = (flags.len() + 1) as i32;
                    self.emit(WasmInstruction::I32Const(pat_ptr as i32));
                    self.emit(WasmInstruction::I32Const(pat_len));
                    self.emit(WasmInstruction::I32Const(flags_ptr as i32));
                    self.emit(WasmInstruction::I32Const(flags_len));
                    let func_idx = self
                        .builtin_func_indices
                        .get(&Builtin::RegExpCreate)
                        .copied()
                        .ok_or_else(|| anyhow::anyhow!("no WASM func index for RegExpCreate"))?;
                    self.emit(WasmInstruction::Call(func_idx));
                    self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                } else {
                    let encoded = self.encode_constant(constant, module)?;
                    self.emit(WasmInstruction::I64Const(encoded));
                    self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                }
                Ok(false)
            }
            Instruction::Binary { dest, op, lhs, rhs } => {
                match op {
                    // 加法：先尝试字符串连接，失败再做数值加法
                    BinaryOp::Add => {
                        // 调用 string_concat(lhs, rhs)
                        self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                        self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                        self.emit(WasmInstruction::Call(
                            self.special_host_import_indices[&SpecialHostImport::StringConcat],
                        ));
                        // 存到 scratch
                        self.emit(WasmInstruction::LocalSet(self.string_concat_scratch_idx));
                        // 检查结果是否为 undefined（哨兵值：表示无字符串操作数）
                        self.emit(WasmInstruction::LocalGet(self.string_concat_scratch_idx));
                        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                        self.emit(WasmInstruction::I64Eq);
                        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
                        // 结果是 undefined → 走数值加法 (F64Add)
                        self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                        self.emit(WasmInstruction::F64ReinterpretI64);
                        self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                        self.emit(WasmInstruction::F64ReinterpretI64);
                        self.emit(WasmInstruction::F64Add);
                        self.emit(WasmInstruction::I64ReinterpretF64);
                        self.emit(WasmInstruction::Else);
                        // 结果是字符串 → 直接使用
                        self.emit(WasmInstruction::LocalGet(self.string_concat_scratch_idx));
                        self.emit(WasmInstruction::End);
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    // 算术：两操作数均为 BigInt 时走 bigint_* host（除法截断 toward zero）
                    BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                        let lhs_l = self.local_idx(lhs.0);
                        let rhs_l = self.local_idx(rhs.0);
                        let (bigint_builtin, f64_op) = match op {
                            BinaryOp::Sub => (Builtin::BigIntSub, WasmInstruction::F64Sub),
                            BinaryOp::Mul => (Builtin::BigIntMul, WasmInstruction::F64Mul),
                            BinaryOp::Div => (Builtin::BigIntDiv, WasmInstruction::F64Div),
                            _ => unreachable!(),
                        };
                        self.emit_bigint_or_f64_binary(lhs_l, rhs_l, bigint_builtin, f64_op)?;
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    // 位运算（i32 操作）
                    BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor => {
                        // 左操作数：ToNumber → ToInt32
                        self.emit_to_number(self.local_idx(lhs.0))?;
                        self.emit(WasmInstruction::Call(self.to_int32_func_idx));
                        // 右操作数：ToNumber → ToInt32
                        self.emit_to_number(self.local_idx(rhs.0))?;
                        self.emit(WasmInstruction::Call(self.to_int32_func_idx));
                        // 执行位运算
                        match op {
                            BinaryOp::BitAnd => self.emit(WasmInstruction::I32And),
                            BinaryOp::BitOr => self.emit(WasmInstruction::I32Or),
                            BinaryOp::BitXor => self.emit(WasmInstruction::I32Xor),
                            _ => unreachable!(),
                        }
                        // 转换回 Number
                        self.emit(WasmInstruction::F64ConvertI32S);
                        self.emit(WasmInstruction::I64ReinterpretF64);
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    // 移位运算（需要掩码右操作数）
                    BinaryOp::Shl | BinaryOp::Shr | BinaryOp::UShr => {
                        // 左操作数：ToNumber → ToInt32
                        self.emit_to_number(self.local_idx(lhs.0))?;
                        self.emit(WasmInstruction::Call(self.to_int32_func_idx));
                        // 右操作数：ToNumber → ToInt32 并掩码 0x1F
                        self.emit_to_number(self.local_idx(rhs.0))?;
                        self.emit(WasmInstruction::Call(self.to_int32_func_idx));
                        self.emit(WasmInstruction::I32Const(0x1F));
                        self.emit(WasmInstruction::I32And);
                        // 执行移位
                        match op {
                            BinaryOp::Shl => self.emit(WasmInstruction::I32Shl),
                            BinaryOp::Shr => self.emit(WasmInstruction::I32ShrS),
                            BinaryOp::UShr => self.emit(WasmInstruction::I32ShrU),
                            _ => unreachable!(),
                        }
                        // 转换回 Number
                        if matches!(op, BinaryOp::UShr) {
                            self.emit(WasmInstruction::F64ConvertI32U);
                        } else {
                            self.emit(WasmInstruction::F64ConvertI32S);
                        }
                        self.emit(WasmInstruction::I64ReinterpretF64);
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    BinaryOp::Mod | BinaryOp::Exp => {
                        let lhs_l = self.local_idx(lhs.0);
                        let rhs_l = self.local_idx(rhs.0);
                        let (bigint_builtin, f64_builtin) = match op {
                            BinaryOp::Mod => (Builtin::BigIntMod, Builtin::F64Mod),
                            BinaryOp::Exp => (Builtin::BigIntPow, Builtin::F64Exp),
                            _ => unreachable!(),
                        };
                        self.emit_bigint_or_f64_host_binary(lhs_l, rhs_l, bigint_builtin, f64_builtin)?;
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                }
                Ok(false)
            }
            Instruction::Unary { dest, op, value } => {
                match op {
                    UnaryOp::Not => {
                        self.emit_to_bool_i32(value.0);
                        self.emit(WasmInstruction::I32Const(1));
                        self.emit(WasmInstruction::I32Xor);
                        self.emit(WasmInstruction::I64ExtendI32U);
                        let box_base = value::BOX_BASE as i64;
                        let tag_bool = (value::TAG_BOOL << 32) as i64;
                        self.emit(WasmInstruction::I64Const(box_base | tag_bool));
                        self.emit(WasmInstruction::I64Or);
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    UnaryOp::Neg => {
                        let bigint_neg_idx = self
                            .builtin_func_indices
                            .get(&Builtin::BigIntNeg)
                            .copied()
                            .context("no WASM func index for BigIntNeg")?;
                        self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                        self.emit(WasmInstruction::I64Const(32));
                        self.emit(WasmInstruction::I64ShrU);
                        self.emit(WasmInstruction::I64Const(0x1F));
                        self.emit(WasmInstruction::I64And);
                        self.emit(WasmInstruction::I64Const(value::TAG_BIGINT as i64));
                        self.emit(WasmInstruction::I64Eq);
                        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
                        self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                        self.emit(WasmInstruction::Call(bigint_neg_idx));
                        self.emit(WasmInstruction::Else);
                        self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                        self.emit(WasmInstruction::F64ReinterpretI64);
                        self.emit(WasmInstruction::F64Neg);
                        self.emit(WasmInstruction::I64ReinterpretF64);
                        self.emit(WasmInstruction::End);
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    UnaryOp::Pos => {
                        self.emit_to_number(self.local_idx(value.0))?;
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    UnaryOp::BitNot => {
                        // ~x: ToInt32(x) XOR 0xFFFFFFFF
                        // 1. ToNumber → ToInt32(x)
                        self.emit_to_number(self.local_idx(value.0))?;
                        self.emit(WasmInstruction::Call(self.to_int32_func_idx));
                        // 2. XOR with -1 (all ones)
                        self.emit(WasmInstruction::I32Const(-1));
                        self.emit(WasmInstruction::I32Xor);
                        // 3. Convert back to Number (f64) and NaN-box
                        self.emit(WasmInstruction::F64ConvertI32S);
                        self.emit(WasmInstruction::I64ReinterpretF64);
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    UnaryOp::Void => {
                        let _ = value;
                        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    UnaryOp::IsNullish => {
                        self.emit_is_nullish_i32(value.0);
                        self.emit(WasmInstruction::I64ExtendI32U);
                        let box_base = value::BOX_BASE as i64;
                        let tag_bool = (value::TAG_BOOL << 32) as i64;
                        self.emit(WasmInstruction::I64Const(box_base | tag_bool));
                        self.emit(WasmInstruction::I64Or);
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    UnaryOp::Delete => {
                        // delete 操作符在语义层被转换为 DeleteProp 或 Const(true)
                        // 这里不应该被到达
                        bail!(
                            "UnaryOp::Delete should not be reached - delete is handled by DeleteProp instruction"
                        );
                    }
                }
                Ok(false)
            }
            Instruction::Compare { dest, op, lhs, rhs } => {
                self.compile_compare(*dest, *op, *lhs, *rhs).map(|_| false)
            }
            Instruction::Phi { dest, .. } => {
                let phi_local = self
                    .phi_locals
                    .get(&dest.0)
                    .copied()
                    .with_context(|| format!("phi {dest} has no assigned WASM local"))?;

                self.emit(WasmInstruction::LocalGet(phi_local));
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(false)
            }
            Instruction::CallBuiltin {
                dest,
                builtin,
                args,
            } => {
                // GC safepoint（P2）：spill live handles 再调 builtin。
                let spill = self.current_spill_locals();
                self.emit_safepoint_spill_prologue(&spill);
                self.compile_builtin_call(*dest, builtin, args)?;
                self.emit_safepoint_spill_epilogue(spill.len());
                Ok(false)
            }
            Instruction::LoadVar { dest, name } => {
                if let Some(offset) = self.var_memory_offsets.get(name).copied() {
                    self.emit_eval_var_address(offset);
                    self.emit(WasmInstruction::I64Load(MemArg {
                        offset: 0,
                        align: 3,
                        memory_index: 0,
                    }));
                } else {
                    let local_idx = self
                        .var_locals
                        .get(name)
                        .with_context(|| format!("variable `{name}` has no assigned WASM local"))?;
                    self.emit(WasmInstruction::LocalGet(*local_idx));
                }
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(false)
            }
            Instruction::StoreVar { name, value } => {
                if let Some(offset) = self.var_memory_offsets.get(name).copied() {
                    self.emit_eval_var_address(offset);
                    self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                    self.emit(WasmInstruction::I64Store(MemArg {
                        offset: 0,
                        align: 3,
                        memory_index: 0,
                    }));
                } else {
                    let local_idx = *self
                        .var_locals
                        .get(name)
                        .with_context(|| format!("variable `{name}` has no assigned WASM local"))?;
                    self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                    self.emit(WasmInstruction::LocalSet(local_idx));
                }
                Ok(false)
            }
            Instruction::Call {
                dest,
                callee,
                this_val,
                args,
            } => {
                // Layer 3d: 检查 callee 是否可能触发 GC
                // 如果 callee 是已知 no-GC 函数，可省 safepoint spill
                let may_gc = if let Some(func_id) = self.current_function_id {
                    if let Some(ref analysis) = self.gc_analysis {
                        analysis.call_may_trigger_gc(func_id, *callee)
                    } else {
                        true // 无分析结果，保守 spill
                    }
                } else {
                    true // 模块入口函数，保守 spill
                };

                if may_gc {
                    let spill = self.current_spill_locals();
                    self.emit_safepoint_spill_prologue(&spill);
                    self.compile_call_with_new_target(dest, *callee, *this_val, args, None)?;
                    self.emit_safepoint_spill_epilogue(spill.len());
                } else {
                    // no-GC callee: 省掉 safepoint spill
                    self.compile_call_with_new_target(dest, *callee, *this_val, args, None)?;
                }
                Ok(false)
            }
            Instruction::SuperCall {
                dest,
                callee,
                this_val,
                args,
                forward_args,
            } => {
                // SuperCall 保守保留 spill（构造调用几乎必分配）
                let spill = self.current_spill_locals();
                self.emit_safepoint_spill_prologue(&spill);
                self.compile_super_call(dest, *callee, *this_val, args, *forward_args)?;
                self.emit_safepoint_spill_epilogue(spill.len());
                Ok(false)
            }
            Instruction::ConstructCall {
                callee,
                this_val,
                args,
            } => {
                // ConstructCall 保守保留 spill（构造调用几乎必分配）
                let spill = self.current_spill_locals();
                self.emit_safepoint_spill_prologue(&spill);
                self.compile_call_with_new_target(&None, *callee, *this_val, args, Some(*callee))?;
                self.emit_safepoint_spill_epilogue(spill.len());
                Ok(false)
            }
            Instruction::NewObject { dest, capacity } => {
                // GC safepoint（P2）：spill live handles，调 $obj_new，复位 shadow_sp。
                let spill = self.current_spill_locals();
                self.emit_safepoint_spill_prologue(&spill);
                // Call $obj_new(capacity)
                self.emit(WasmInstruction::I32Const(*capacity as i32));
                self.emit(WasmInstruction::Call(self.obj_new_func_idx));
                self.emit_safepoint_spill_epilogue(spill.len());
                // Result is i32 ptr — encode as object handle.
                // object_handle = BOX_BASE | (TAG_OBJECT << 32) | ptr
                self.emit(WasmInstruction::I64ExtendI32U);
                let box_base = value::BOX_BASE as i64;
                let tag_object = (value::TAG_OBJECT << 32) as i64;
                self.emit(WasmInstruction::I64Const(box_base | tag_object));
                self.emit(WasmInstruction::I64Or);
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(false)
            }
            Instruction::GetProp { dest, object, key } => {
                // Pass full boxed i64 value — helper resolves tag internally.
                self.emit(WasmInstruction::LocalGet(self.local_idx(object.0)));
                // Key: lower 32 bits (string pointer or name_id).
                self.emit(WasmInstruction::LocalGet(self.local_idx(key.0)));
                self.emit(WasmInstruction::Call(
                    self.special_host_import_indices[&SpecialHostImport::SymbolPropertyKey],
                ));
                // Call $obj_get(boxed, name_id) -> i64
                self.emit(WasmInstruction::Call(self.obj_get_func_idx));
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(false)
            }
            Instruction::SetProp { object, key, value } => {
                // Pass full boxed i64 value — helper resolves tag internally.
                self.emit(WasmInstruction::LocalGet(self.local_idx(object.0)));
                // Key.
                self.emit(WasmInstruction::LocalGet(self.local_idx(key.0)));
                self.emit(WasmInstruction::Call(
                    self.special_host_import_indices[&SpecialHostImport::SymbolPropertyKey],
                ));
                // Value (i64 NaN-boxed).
                self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                // Call $obj_set(boxed, name_id, value)
                self.emit(WasmInstruction::Call(self.obj_set_func_idx));
                Ok(false)
            }
            Instruction::DeleteProp { dest, object, key } => {
                // delete obj.prop -> bool (成功删除返回 true)
                self.emit(WasmInstruction::LocalGet(self.local_idx(object.0)));
                // Key: lower 32 bits.
                self.emit(WasmInstruction::LocalGet(self.local_idx(key.0)));
                self.emit(WasmInstruction::Call(
                    self.special_host_import_indices[&SpecialHostImport::SymbolPropertyKey],
                ));
                // Call $obj_delete(boxed, name_id) -> i64 (NaN-boxed bool)
                self.emit(WasmInstruction::Call(self.obj_delete_func_idx));
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(false)
            }
            Instruction::SetProto { object, value } => {
                // 验证 value 是有效的对象/函数引用后再设置 __proto__
                // 条件: is_boxed(value) AND (tag == OBJECT OR tag == FUNCTION)
                let val_local = self.local_idx(value.0);
                let obj_local = self.local_idx(object.0);
                let box_base = value::BOX_BASE as i64;

                // (1) is_boxed: (val & BOX_BASE) == BOX_BASE → i32
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I64Const(box_base));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(box_base));
                self.emit(WasmInstruction::I64Eq);

                // (2) tag == OBJECT: ((val >> 32) & TAG_MASK) == TAG_OBJECT → i32
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(value::TAG_MASK as i64));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_OBJECT as i64));
                self.emit(WasmInstruction::I64Eq);

                // (3) tag == FUNCTION: ((val >> 32) & TAG_MASK) == TAG_FUNCTION → i32
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(value::TAG_MASK as i64));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_FUNCTION as i64));
                self.emit(WasmInstruction::I64Eq);

                // (4) tag == TAG_CLOSURE: ((val >> 32) & TAG_MASK) == TAG_CLOSURE → i32
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(value::TAG_MASK as i64));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_CLOSURE as i64));
                self.emit(WasmInstruction::I64Eq);

                // (5) tag == TAG_ARRAY: ((val >> 32) & TAG_MASK) == TAG_ARRAY → i32
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(value::TAG_MASK as i64));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_ARRAY as i64));
                self.emit(WasmInstruction::I64Eq);

                // (6) tag == TAG_BOUND: ((val >> 32) & TAG_MASK) == TAG_BOUND → i32
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(value::TAG_MASK as i64));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_BOUND as i64));
                self.emit(WasmInstruction::I64Eq);

                // (7) tag == TAG_PROXY: ((val >> 32) & TAG_MASK) == TAG_PROXY → i32
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(value::TAG_MASK as i64));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_PROXY as i64));
                self.emit(WasmInstruction::I64Eq);

                // (2) OR (3) OR (4) OR (5) OR (6) OR (7): tag_valid → i32
                self.emit(WasmInstruction::I32Or);
                self.emit(WasmInstruction::I32Or);
                self.emit(WasmInstruction::I32Or);
                self.emit(WasmInstruction::I32Or);
                self.emit(WasmInstruction::I32Or);
                // (1) AND tag_valid: combined → i32
                self.emit(WasmInstruction::I32And);

                // 条件分支：仅当 tag 有效时执行 __proto__ 存储
                // 需要通过 handle 表解析 obj 和 value 的真实 ptr
                self.emit(WasmInstruction::If(BlockType::Empty));
                // 解析 obj handle → real obj ptr；函数对象的属性对象从 __function_props_base 起算。
                self.emit(WasmInstruction::LocalGet(obj_local));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(value::TAG_MASK as i64));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_FUNCTION as i64));
                self.emit(WasmInstruction::I64Eq);
                self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
                self.emit(WasmInstruction::LocalGet(obj_local));
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::GlobalGet(
                    self.function_props_base_global_idx,
                ));
                self.emit(WasmInstruction::I32Add);
                self.emit(WasmInstruction::Else);
                self.emit(WasmInstruction::LocalGet(obj_local));
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::End);
                self.emit(WasmInstruction::I32Const(4));
                self.emit(WasmInstruction::I32Mul);
                self.emit(WasmInstruction::GlobalGet(self.obj_table_global_idx));
                self.emit(WasmInstruction::I32Add);
                self.emit(WasmInstruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                // value 为函数时同样把函数表索引重定位到函数属性 handle。
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(value::TAG_MASK as i64));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_FUNCTION as i64));
                self.emit(WasmInstruction::I64Eq);
                self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::GlobalGet(
                    self.function_props_base_global_idx,
                ));
                self.emit(WasmInstruction::I32Add);
                self.emit(WasmInstruction::Else);
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::End);
                // 存储：obj[0] = value_handle_idx
                self.emit(WasmInstruction::I32Store(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                self.emit(WasmInstruction::End);
                Ok(false)
            }
            Instruction::NewArray { dest, capacity } => {
                // GC safepoint（P2）：spill live handles，调 $arr_new，复位 shadow_sp。
                let spill = self.current_spill_locals();
                self.emit_safepoint_spill_prologue(&spill);
                // Call $arr_new(capacity) -> i32 (handle index)
                self.emit(WasmInstruction::I32Const(*capacity as i32));
                self.emit(WasmInstruction::Call(self.arr_new_func_idx));
                self.emit_safepoint_spill_epilogue(spill.len());
                // Encode as array handle: BOX_BASE | (TAG_ARRAY << 32) | handle
                self.emit(WasmInstruction::I64ExtendI32U);
                let box_base = value::BOX_BASE as i64;
                let tag_array = (value::TAG_ARRAY << 32) as i64;
                self.emit(WasmInstruction::I64Const(box_base | tag_array));
                self.emit(WasmInstruction::I64Or);
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(false)
            }
            Instruction::GetElem {
                dest,
                object,
                index,
            } => {
                // 按 key 运行期类型分派（见 emit_computed_get）：数字→$elem_get；字符串/symbol→命名属性或数组规范索引。
                self.emit_computed_get(*object, *index);
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(false)
            }
            Instruction::SetElem {
                object,
                index,
                value,
            } => {
                // 按 key 运行期类型分派（见 emit_computed_set）：数字→$elem_set；字符串/symbol→命名属性或数组规范索引。
                self.emit_computed_set(*object, *index, *value);
                Ok(false)
            }
            Instruction::StringConcatVa { dest, parts } => {
                // P4-b4 safepoint：string_concat_va host 产 runtime string handle（alloc）。
                // 注：compile_string_concat_va 内部自管 shadow_sp（push parts + restore），
                // spill prologue/epilogue 用独立 safepoint_sp_saved_idx，不冲突。
                // spill 在 compile_string_concat_va 入口前发生，其内部 push 在 spill 之上，
                // epilogue 在其 restore 后恢复 shadow_sp 到 spill 前。
                let spill = self.current_spill_locals();
                self.emit_safepoint_spill_prologue(&spill);
                let r = self.compile_string_concat_va(dest, parts);
                self.emit_safepoint_spill_epilogue(spill.len());
                r.map(|_| false)
            }
            Instruction::OptionalGetProp { dest, object, key } => self
                .compile_optional_get(dest, object, true, Some(key), false)
                .map(|_| false),
            Instruction::OptionalGetElem { dest, object, key } => self
                .compile_optional_get(dest, object, false, Some(key), false)
                .map(|_| false),
            Instruction::OptionalCall {
                dest,
                callee,
                this_val,
                args,
            } => self
                .compile_optional_call(dest, callee, this_val, args)
                .map(|_| false),
            Instruction::ObjectSpread { dest, source } => {
                // P4-b4 safepoint：ObjSpread host alloc 可能触发 GC，spill live handles。
                let spill = self.current_spill_locals();
                self.emit_safepoint_spill_prologue(&spill);
                let r = self.compile_object_spread(dest, source);
                self.emit_safepoint_spill_epilogue(spill.len());
                r.map(|_| false)
            }
            Instruction::GetSuperBase { dest } => self.compile_get_super_base(dest).map(|_| false),
            Instruction::GetSuperConstructor { dest } => {
                self.compile_get_super_constructor(dest).map(|_| false)
            }
            Instruction::NewPromise { dest } => {
                // P4-b4 safepoint：promise_create host alloc 可能触发 GC，spill live handles。
                let spill = self.current_spill_locals();
                self.emit_safepoint_spill_prologue(&spill);
                let func_idx = self.builtin_func_indices[&Builtin::PromiseCreate];
                self.emit(WasmInstruction::I64Const(0));
                self.emit(WasmInstruction::Call(func_idx));
                self.emit_safepoint_spill_epilogue(spill.len());
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(false)
            }
            Instruction::PromiseResolve { promise, value } => {
                // P4-b4 safepoint：host 可能 alloc（reaction/natives）。
                let spill = self.current_spill_locals();
                self.emit_safepoint_spill_prologue(&spill);
                let func_idx = self.builtin_func_indices[&Builtin::PromiseInstanceResolve];
                self.emit(WasmInstruction::LocalGet(self.local_idx(promise.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                self.emit(WasmInstruction::Call(func_idx));
                self.emit_safepoint_spill_epilogue(spill.len());
                Ok(false)
            }
            Instruction::PromiseReject { promise, reason } => {
                // P4-b4 safepoint：host 可能 alloc（reaction/natives）。
                let spill = self.current_spill_locals();
                self.emit_safepoint_spill_prologue(&spill);
                let func_idx = self.builtin_func_indices[&Builtin::PromiseInstanceReject];
                self.emit(WasmInstruction::LocalGet(self.local_idx(promise.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(reason.0)));
                self.emit(WasmInstruction::Call(func_idx));
                self.emit_safepoint_spill_epilogue(spill.len());
                Ok(false)
            }
            Instruction::Suspend { promise, state } => {
                let func_idx = self.builtin_func_indices[&Builtin::AsyncFunctionSuspend];
                self.emit(WasmInstruction::LocalGet(self.continuation_local_idx));
                self.emit(WasmInstruction::LocalGet(self.local_idx(promise.0)));
                self.emit(WasmInstruction::I64Const(*state as i64));
                self.emit(WasmInstruction::Call(func_idx));
                if self.current_func_returns_value {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                self.emit(WasmInstruction::Return);
                Ok(true)
            }
            Instruction::CollectRestArgs { dest, skip } => {
                // P4-b4 safepoint：arr_new 在此 handler 内调用（alloc，可能触发 GC）。
                // 循环内 ArrayPush 经 grow_array 分配但不主动触发 GC（无 gc_maybe_collect），
                // 故顶层 spill 覆盖 arr_new 处的 live handle locals 即可。
                let spill = self.current_spill_locals();
                self.emit_safepoint_spill_prologue(&spill);
                let skip_val = *skip as i32;
                let arr_push_func_idx = self.builtin_func_indices[&Builtin::ArrayPush];

                self.emit(WasmInstruction::LocalGet(3));
                self.emit(WasmInstruction::I32Const(skip_val));
                self.emit(WasmInstruction::I32Sub);
                self.emit(WasmInstruction::LocalTee(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::I32Const(0));
                self.emit(WasmInstruction::I32LtS);
                self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
                self.emit(WasmInstruction::I32Const(0));
                self.emit(WasmInstruction::Else);
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::End);
                self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));

                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::I32Const(1));
                self.emit(WasmInstruction::I32LtS);
                self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
                self.emit(WasmInstruction::I32Const(1));
                self.emit(WasmInstruction::Else);
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::End);
                self.emit(WasmInstruction::Call(self.arr_new_func_idx));
                // Result is i32 ptr — encode as array handle
                self.emit(WasmInstruction::I64ExtendI32U);
                let box_base = value::BOX_BASE as i64;
                let tag_array = (value::TAG_ARRAY << 32) as i64;
                self.emit(WasmInstruction::I64Const(box_base | tag_array));
                self.emit(WasmInstruction::I64Or);
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));

                // Loop: for i in 0..rest_count, load arg from shadow stack and ArrayPush
                let loop_counter = self.call_func_idx_scratch();
                self.emit(WasmInstruction::I32Const(0));
                self.emit(WasmInstruction::LocalSet(loop_counter));

                self.emit(WasmInstruction::Block(BlockType::Empty));
                self.emit(WasmInstruction::Loop(BlockType::Empty));
                // Check: loop_counter < rest_count
                self.emit(WasmInstruction::LocalGet(loop_counter));
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::I32GeU);
                self.emit(WasmInstruction::BrIf(1));

                // Load arg from shadow stack: args_base + (skip + loop_counter) * 8
                self.emit(WasmInstruction::LocalGet(2)); // args_base
                self.emit(WasmInstruction::I32Const(skip_val));
                self.emit(WasmInstruction::LocalGet(loop_counter));
                self.emit(WasmInstruction::I32Add);
                self.emit(WasmInstruction::I32Const(3)); // * 8
                self.emit(WasmInstruction::I32Shl);
                self.emit(WasmInstruction::I32Add);
                self.emit(WasmInstruction::I64Load(MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                }));
                // Stack: [arg_value]
                // Save arg_value
                self.emit(WasmInstruction::LocalSet(self.call_env_obj_scratch()));

                // Call ArrayPush(arr_handle, arg_value)
                self.emit(WasmInstruction::LocalGet(self.local_idx(dest.0)));
                self.emit(WasmInstruction::LocalGet(self.call_env_obj_scratch()));
                self.emit(WasmInstruction::Call(arr_push_func_idx));
                self.emit(WasmInstruction::Drop);

                // Increment loop counter
                self.emit(WasmInstruction::LocalGet(loop_counter));
                self.emit(WasmInstruction::I32Const(1));
                self.emit(WasmInstruction::I32Add);
                self.emit(WasmInstruction::LocalSet(loop_counter));

                self.emit(WasmInstruction::Br(0));
                self.emit(WasmInstruction::End); // end loop
                self.emit(WasmInstruction::End); // end block

                self.emit_safepoint_spill_epilogue(spill.len());
                Ok(false)
            }
            Instruction::IsException { dest, value } => {
                let box_base = value::BOX_BASE as i64;
                let tag_exception = value::TAG_EXCEPTION as i64;
                let tag_mask = value::TAG_MASK as i64;
                let bool_true = value::encode_bool(true);
                let bool_false = value::encode_bool(false);

                // Check BOX_BASE
                self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                self.emit(WasmInstruction::I64Const(box_base));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(box_base));
                self.emit(WasmInstruction::I64Ne);
                self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
                self.emit(WasmInstruction::I64Const(bool_false));
                self.emit(WasmInstruction::Else);
                // Check tag
                self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(tag_mask));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(tag_exception));
                self.emit(WasmInstruction::I64Eq);
                self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
                self.emit(WasmInstruction::I64Const(bool_true));
                self.emit(WasmInstruction::Else);
                self.emit(WasmInstruction::I64Const(bool_false));
                self.emit(WasmInstruction::End);
                self.emit(WasmInstruction::End);
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(false)
            }
            Instruction::EncodeException { dest, value } => {
                let box_base = value::BOX_BASE as i64;
                let tag_exception = value::TAG_EXCEPTION as i64;
                let encoded_base = box_base | ((tag_exception & 0x1F) << 32);
                self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                self.emit(WasmInstruction::I64Const(0xFFFFFFFFi64));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(encoded_base));
                self.emit(WasmInstruction::I64Or);
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(false)
            }
            Instruction::ExceptionToObject { dest, value } => {
                let box_base = value::BOX_BASE as i64;
                let tag_object = value::TAG_OBJECT as i64;
                let decoded_base = box_base | ((tag_object & 0x1F) << 32);
                self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                self.emit(WasmInstruction::I64Const(0xFFFFFFFFi64));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(decoded_base));
                self.emit(WasmInstruction::I64Or);
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(false)
            }
        }
    }

}
