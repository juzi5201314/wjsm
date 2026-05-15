use anyhow::{Context, Result, bail};
use wasm_encoder::{BlockType, Instruction as WasmInstruction, MemArg, ValType};
use wjsm_ir::{
    BinaryOp, Builtin, CompareOp, Constant, Instruction,
    Module as IrModule, UnaryOp, ValueId, value,
};

use super::state::Compiler;

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
                        .unwrap_or(95);
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
                        .unwrap_or(109);
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
                        self.emit(WasmInstruction::Call(16)); // import 16: string_concat
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
                    // 其他算术运算（f64 操作）
                    BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                        self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                        self.emit(WasmInstruction::F64ReinterpretI64);
                        self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                        self.emit(WasmInstruction::F64ReinterpretI64);
                        match op {
                            BinaryOp::Sub => self.emit(WasmInstruction::F64Sub),
                            BinaryOp::Mul => self.emit(WasmInstruction::F64Mul),
                            BinaryOp::Div => self.emit(WasmInstruction::F64Div),
                            _ => unreachable!(),
                        }
                        self.emit(WasmInstruction::I64ReinterpretF64);
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    // 位运算（i32 操作）
                    BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor => {
                        // 左操作数：ToInt32
                        self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                        self.emit(WasmInstruction::Call(self.to_int32_func_idx));
                        // 右操作数：ToInt32
                        self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
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
                        // 左操作数：ToInt32
                        self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                        self.emit(WasmInstruction::Call(self.to_int32_func_idx));
                        // 右操作数：ToInt32 并掩码 0x1F
                        self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
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
                        bail!("Mod/Exp should be lowered to CallBuiltin, not Binary op");
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
                        self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                        self.emit(WasmInstruction::F64ReinterpretI64);
                        self.emit(WasmInstruction::F64Neg);
                        self.emit(WasmInstruction::I64ReinterpretF64);
                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    UnaryOp::Pos => {
                        // +x 应执行 ToNumber(x):
                        //   f64 → 原值; null → 0; true → 1; false → 0;
                        //   undefined / string / object / 其他 → NaN
                        let val_local = self.local_idx(value.0);
                        let box_base = value::BOX_BASE as i64;

                        // 检查是否为 NaN-boxed 值
                        self.emit(WasmInstruction::LocalGet(val_local));
                        self.emit(WasmInstruction::I64Const(box_base));
                        self.emit(WasmInstruction::I64And);
                        self.emit(WasmInstruction::I64Const(box_base));
                        self.emit(WasmInstruction::I64Eq);

                        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
                        // boxed: 按 tag 分派
                        // 提取 tag
                        self.emit(WasmInstruction::LocalGet(val_local));
                        self.emit(WasmInstruction::I64Const(32));
                        self.emit(WasmInstruction::I64ShrU);
                        self.emit(WasmInstruction::I64Const(0xF));
                        self.emit(WasmInstruction::I64And);
                        // TAG_NULL?
                        self.emit(WasmInstruction::I64Const(value::TAG_NULL as i64));
                        self.emit(WasmInstruction::I64Eq);
                        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
                        // null → +0
                        self.emit(WasmInstruction::I64Const(0)); // encode_f64(0.0)
                        self.emit(WasmInstruction::Else);
                        // 提取 tag
                        self.emit(WasmInstruction::LocalGet(val_local));
                        self.emit(WasmInstruction::I64Const(32));
                        self.emit(WasmInstruction::I64ShrU);
                        self.emit(WasmInstruction::I64Const(0xF));
                        self.emit(WasmInstruction::I64And);
                        // TAG_BOOL?
                        self.emit(WasmInstruction::I64Const(value::TAG_BOOL as i64));
                        self.emit(WasmInstruction::I64Eq);
                        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
                        // boolean: 检查 payload
                        self.emit(WasmInstruction::LocalGet(val_local));
                        self.emit(WasmInstruction::I64Const(1));
                        self.emit(WasmInstruction::I64And);
                        self.emit(WasmInstruction::I64Const(1));
                        self.emit(WasmInstruction::I64Eq);
                        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
                        // true → 1.0
                        self.emit(WasmInstruction::I64Const(1.0f64.to_bits() as i64));
                        self.emit(WasmInstruction::Else);
                        // false → 0.0
                        self.emit(WasmInstruction::I64Const(0));
                        self.emit(WasmInstruction::End);
                        self.emit(WasmInstruction::Else);
                        // 其他 boxed 类型 → NaN
                        self.emit(WasmInstruction::I64Const(value::BOX_BASE as i64));
                        self.emit(WasmInstruction::End);
                        self.emit(WasmInstruction::End);
                        self.emit(WasmInstruction::Else);
                        // not boxed → raw f64, 返回原值
                        self.emit(WasmInstruction::LocalGet(val_local));
                        self.emit(WasmInstruction::End);

                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                    }
                    UnaryOp::BitNot => {
                        // ~x: ToInt32(x) XOR 0xFFFFFFFF
                        // 1. Load value and convert to i32 (ToInt32)
                        self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
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
            } => self
                .compile_builtin_call(*dest, builtin, args)
                .map(|_| false),
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
                // 使用影子栈传递参数
                // Type 12 签名: (i64 env_obj, i64 this_val, i32 args_base, i32 args_count) -> i64
                // callee 可能是 TAG_FUNCTION 或 TAG_CLOSURE，运行时解析

                // Step 1: 保存 shadow_sp 到 scratch local
                self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
                self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));

                // Step 1b: 影子栈边界检查
                self.emit_shadow_stack_overflow_check((args.len() * 8) as i32);

                // Step 2: 将所有参数写入影子栈
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

                // Step 3: native callable 由宿主运行时执行，普通 JS 函数继续走函数表。
                let call_func_idx_scratch = self.call_func_idx_scratch();
                let call_env_obj_scratch = self.call_env_obj_scratch();

                self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(0xF));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_NATIVE_CALLABLE as i64));
                self.emit(WasmInstruction::I64Eq);
                self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(this_val.0)));
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::I32Const(args.len() as i32));
                self.emit(WasmInstruction::Call(self.native_call_func_idx));
                self.emit(WasmInstruction::Else);

                // 运行时解析 callee → (func_idx, env_obj)。callee 可能是 TAG_FUNCTION 或 TAG_CLOSURE。
                self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(0xF));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(0xA)); // TAG_CLOSURE
                self.emit(WasmInstruction::I64Eq);
                self.emit(WasmInstruction::If(BlockType::Empty));
                self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::Call(self.closure_get_func_idx));
                self.emit(WasmInstruction::LocalSet(call_func_idx_scratch));
                self.emit(WasmInstruction::LocalGet(self.local_idx(callee.0)));
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::Call(self.closure_get_env_idx));
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
                self.emit(WasmInstruction::End);

                // Step 4: 恢复 shadow_sp
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));

                // Step 5: 处理返回值
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                } else {
                    self.emit(WasmInstruction::Drop);
                }
                Ok(false)
            }
            Instruction::NewObject { dest, capacity } => {
                // Call $obj_new(capacity)
                self.emit(WasmInstruction::I32Const(*capacity as i32));
                self.emit(WasmInstruction::Call(self.obj_new_func_idx));
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
                self.emit(WasmInstruction::I32WrapI64);
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
                self.emit(WasmInstruction::I32WrapI64);
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
                self.emit(WasmInstruction::I32WrapI64);
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

                // (2) tag == OBJECT: ((val >> 32) & 0xF) == TAG_OBJECT → i32
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(0xF));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_OBJECT as i64));
                self.emit(WasmInstruction::I64Eq);

                // (3) tag == FUNCTION: ((val >> 32) & 0xF) == TAG_FUNCTION → i32
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(0xF));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_FUNCTION as i64));
                self.emit(WasmInstruction::I64Eq);

                // (2) OR (3): tag_valid → i32
                self.emit(WasmInstruction::I32Or);
                // (1) AND tag_valid: combined → i32
                self.emit(WasmInstruction::I32And);

                // 条件分支：仅当 tag 有效时执行 __proto__ 存储
                // 需要通过 handle 表解析 obj 和 value 的真实 ptr
                self.emit(WasmInstruction::If(BlockType::Empty));
                // 解析 obj handle → real obj ptr
                self.emit(WasmInstruction::LocalGet(obj_local));
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::I32Const(4));
                self.emit(WasmInstruction::I32Mul);
                self.emit(WasmInstruction::GlobalGet(self.obj_table_global_idx));
                self.emit(WasmInstruction::I32Add);
                self.emit(WasmInstruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                // 直接存储 value 的 handle_idx（不需要解析为 ptr）
                // handle_idx = value 的低 32 位
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I32WrapI64);
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
                // Call $arr_new(capacity) -> i32 (handle index)
                self.emit(WasmInstruction::I32Const(*capacity as i32));
                self.emit(WasmInstruction::Call(self.arr_new_func_idx));
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
                // Call $to_int32(index) first (index is an f64), then $elem_get
                self.emit(WasmInstruction::LocalGet(self.local_idx(object.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(index.0)));
                self.emit(WasmInstruction::Call(self.to_int32_func_idx));
                self.emit(WasmInstruction::Call(self.elem_get_func_idx));
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(false)
            }
            Instruction::SetElem {
                object,
                index,
                value,
            } => {
                // Call $to_int32(index) first, then $elem_set
                self.emit(WasmInstruction::LocalGet(self.local_idx(object.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(index.0)));
                self.emit(WasmInstruction::Call(self.to_int32_func_idx));
                self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                self.emit(WasmInstruction::Call(self.elem_set_func_idx));
                Ok(false)
            }
            Instruction::StringConcatVa { dest, parts } => {
                self.compile_string_concat_va(dest, parts).map(|_| false)
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
                self.compile_object_spread(dest, source).map(|_| false)
            }
            Instruction::GetSuperBase { dest } => self.compile_get_super_base(dest).map(|_| false),
            Instruction::NewPromise { dest } => {
                let func_idx = self.builtin_func_indices[&Builtin::PromiseCreate];
                self.emit(WasmInstruction::I64Const(0));
                self.emit(WasmInstruction::Call(func_idx));
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(false)
            }
            Instruction::PromiseResolve { promise, value } => {
                let func_idx = self.builtin_func_indices[&Builtin::PromiseInstanceResolve];
                self.emit(WasmInstruction::LocalGet(self.local_idx(promise.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                self.emit(WasmInstruction::Call(func_idx));
                Ok(false)
            }
            Instruction::PromiseReject { promise, reason } => {
                let func_idx = self.builtin_func_indices[&Builtin::PromiseInstanceReject];
                self.emit(WasmInstruction::LocalGet(self.local_idx(promise.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(reason.0)));
                self.emit(WasmInstruction::Call(func_idx));
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

                Ok(false)
            }
        }
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
}
