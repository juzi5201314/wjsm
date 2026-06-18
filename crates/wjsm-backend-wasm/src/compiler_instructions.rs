use super::*;
use crate::host_import_registry::SpecialHostImport;

impl Compiler {
    /// 设置当前 emit 位置的 IR block/instruction cursor（P2 safepoint spill 用）。
    /// 在 compile_instruction 调用前设置，使 alloc 指令能查到正确的 liveness。
    pub(crate) fn set_emit_cursor(&mut self, block_idx: usize, instr_idx: usize) {
        self.current_emit_block_idx = block_idx;
        self.current_emit_instr_idx = instr_idx;
    }

    /// 判断指令是否为 GC safepoint（可能触发分配或 GC 的点）。
    /// 这些点的 live handle locals 必须 spill 到 shadow stack，否则 GC 误回收。
    pub(crate) fn is_safepoint(ins: &Instruction) -> bool {
        matches!(
            ins,
            Instruction::NewObject { .. }
                | Instruction::NewArray { .. }
                | Instruction::Call { .. }
                | Instruction::CallBuiltin { .. }
                | Instruction::SuperCall { .. }
                | Instruction::ConstructCall { .. }
                // P4-b4 补全：下列指令也分配（host alloc 或 arr_new/obj_new）
                | Instruction::ObjectSpread { .. }
                | Instruction::CollectRestArgs { .. }
                | Instruction::NewPromise { .. }
                | Instruction::PromiseResolve { .. }
                | Instruction::PromiseReject { .. }
                | Instruction::StringConcatVa { .. }
        )
    }

    /// 返回当前 emit 位置（紧邻当前指令执行前）需 spill 的 local idx 列表。
    /// = live ValueId ∩ Handle 类型（保守：ValueTy 缺失当 Handle）→ local_idx。
    /// 结果已 sort + dedup。
    fn current_spill_locals(&self) -> Vec<u32> {
        let block_id = wjsm_ir::BasicBlockId(self.current_emit_block_idx as u32);
        let mut spill: Vec<u32> = Vec::new();

        // ── SSA 值 spill：存活且 Handle 类型的 ValueId → local（ValueTy 缺失保守当 Handle）──
        if let Some(ref liveness) = self.current_fn_liveness
            && let Some(live) = liveness
                .get(&block_id)
                .and_then(|m| m.get(&self.current_emit_instr_idx))
        {
            let value_ty = self.current_fn_value_ty.as_ref();
            spill.extend(
                live.iter()
                    .filter(|v| {
                        value_ty
                            .and_then(|m| m.get(v))
                            .is_none_or(|t| *t == ValueTy::Handle)
                    })
                    .map(|v| self.local_idx(v.0)),
            );
        }

        // ── 变量 spill：存活且可能持有 handle 的变量 local ──
        // per-ValueId liveness 看不到变量存活（StoreVar 无 ValueId def、LoadVar 无 use），
        // 故 store/load 之间存在 liveness 空洞，handle 仅活在变量 local。这里按变量活跃集 +
        // 变量类型补 spill；标量变量（循环计数器、Math.E 等内建）被 ValueTy 过滤，热循环不退化。
        if let Some(ref var_live) = self.current_fn_var_liveness
            && let Some(names) = var_live
                .get(&block_id)
                .and_then(|m| m.get(&self.current_emit_instr_idx))
        {
            let var_ty = self.current_fn_var_ty.as_ref();
            for name in names {
                let is_handle = var_ty
                    .and_then(|m| m.get(name))
                    .is_none_or(|t| *t == ValueTy::Handle);
                if is_handle
                    && let Some(&local) = self.var_locals.get(name)
                {
                    spill.push(local);
                }
            }
        }

        spill.sort_unstable();
        spill.dedup();
        spill
    }

    /// 计算成员读取 `obj[key]` 按 key 运行期类型分派（结果 i64 留在栈上）：
    /// - key 为数字（is_f64）→ `$elem_get`（数组元素 / typedarray / 对象数字属性 via to_string）。
    /// - key 为字符串/symbol → `symbol_property_key` → `$obj_get`（命名属性，含数组 .length / 原型 / 函数属性）。
    ///
    /// 旧实现把所有 computed key `to_int32` 后只走 `$elem_get`，导致：①`a[变量]` 读到 undefined
    /// （非数字字面量被 lowerer 误判走 GetProp，且 obj_get 无元素路径）；②`o[字符串]` 读写错位
    /// （字符串被 to_int32 成 0）。统一为按 key 类型分派后两者皆正确。
    /// 已知遗留小缺口：`o[5]`（纯数字 key 写普通对象）与 `a["5"]`（数字字符串索引数组）走另一分支，
    /// 属 pre-existing 罕见场景，本次不覆盖。
    fn emit_computed_get(&mut self, object: ValueId, key: ValueId) {
        let box_base = value::BOX_BASE as i64;
        let obj_l = self.local_idx(object.0);
        let key_l = self.local_idx(key.0);
        let to_int32 = self.to_int32_func_idx;
        let elem_get = self.elem_get_func_idx;
        let obj_get = self.obj_get_func_idx;
        let sym_key = self.special_host_import_indices[&SpecialHostImport::SymbolPropertyKey];
        // is_f64(key): (key & BOX_BASE) != BOX_BASE
        self.emit(WasmInstruction::LocalGet(key_l));
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64Ne);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
        self.emit(WasmInstruction::LocalGet(obj_l));
        self.emit(WasmInstruction::LocalGet(key_l));
        self.emit(WasmInstruction::Call(to_int32));
        self.emit(WasmInstruction::Call(elem_get));
        self.emit(WasmInstruction::Else);
        self.emit(WasmInstruction::LocalGet(obj_l));
        self.emit(WasmInstruction::LocalGet(key_l));
        self.emit(WasmInstruction::Call(sym_key));
        self.emit(WasmInstruction::Call(obj_get));
        self.emit(WasmInstruction::End);
    }

    /// 计算成员写入 `obj[key] = value` 按 key 运行期类型分派（见 `emit_computed_get`）。
    fn emit_computed_set(&mut self, object: ValueId, key: ValueId, value: ValueId) {
        let box_base = value::BOX_BASE as i64;
        let obj_l = self.local_idx(object.0);
        let key_l = self.local_idx(key.0);
        let val_l = self.local_idx(value.0);
        let to_int32 = self.to_int32_func_idx;
        let elem_set = self.elem_set_func_idx;
        let obj_set = self.obj_set_func_idx;
        let sym_key = self.special_host_import_indices[&SpecialHostImport::SymbolPropertyKey];
        self.emit(WasmInstruction::LocalGet(key_l));
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64Ne);
        self.emit(WasmInstruction::If(BlockType::Empty));
        self.emit(WasmInstruction::LocalGet(obj_l));
        self.emit(WasmInstruction::LocalGet(key_l));
        self.emit(WasmInstruction::Call(to_int32));
        self.emit(WasmInstruction::LocalGet(val_l));
        self.emit(WasmInstruction::Call(elem_set));
        self.emit(WasmInstruction::Else);
        self.emit(WasmInstruction::LocalGet(obj_l));
        self.emit(WasmInstruction::LocalGet(key_l));
        self.emit(WasmInstruction::Call(sym_key));
        self.emit(WasmInstruction::LocalGet(val_l));
        self.emit(WasmInstruction::Call(obj_set));
        self.emit(WasmInstruction::End);
    }

    /// Safepoint spill prologue：保存 spill 前 shadow_sp，把 live handle locals 写到 shadow stack 顶端，推进 shadow_sp。
    ///
    /// non-moving GC 关键：GC 不改 local 值，故无需 reload。epilogue 恢复 shadow_sp 到保存值。
    /// 用独立 safepoint_sp_saved_idx（i32 local），不占用 shadow_sp_scratch_idx（Call arg-save 用），
    /// 避免与 Call/SuperCall body 内部的 shadow_sp 操作冲突。
    ///
    /// 注：不能用 `shadow_sp -= n*8` 复位——SuperCall forward_args 分支会把 shadow_sp
    /// 重置为 caller args_base（非 spill 前值），subtract 会得到错误结果。save/restore 稳健。
    ///
    /// **Layer 2 batch 优化（7→3 条/值）**：原方案逐值推进 shadow_sp（每值 7 条指令）。
    /// 改用 immediate offset：先把 shadow_sp 存为 spill_base，N 个值全部写到
    /// `base + i*8`（每值 3 条），最后一次性把 shadow_sp 推进 N*8 让 GC 扫到 spilled 值。
    /// 总指令：2（存 base）+ 3N（写 N 值）+ 3（推进 sp）= 3N+5（vs 原 7N+2），N=35 时 110 vs 247。
    fn emit_safepoint_spill_prologue(&mut self, spill: &[u32]) {
        if spill.is_empty() {
            return;
        }
        // 保存 spill 前 shadow_sp 到 safepoint_sp_saved（= spill_base）
        self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
        self.emit(WasmInstruction::LocalSet(self.safepoint_sp_saved_idx));
        // spill each live handle local 到 base + i*8（immediate offset，无需逐值推进 sp）
        for (i, &local) in spill.iter().enumerate() {
            self.emit(WasmInstruction::LocalGet(self.safepoint_sp_saved_idx));
            self.emit(WasmInstruction::LocalGet(local));
            self.emit(WasmInstruction::I64Store(MemArg {
                offset: (i as u64) * 8,
                align: 3,
                memory_index: 0,
            }));
        }
        // 一次性推进 shadow_sp = base + N*8，让 GC 扫到 spilled 值
        self.emit(WasmInstruction::LocalGet(self.safepoint_sp_saved_idx));
        self.emit(WasmInstruction::I32Const((spill.len() * 8) as i32));
        self.emit(WasmInstruction::I32Add);
        self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
    }

    /// Safepoint spill epilogue：恢复 shadow_sp 到 prologue 保存的值（non-moving 无需 reload local）。
    fn emit_safepoint_spill_epilogue(&mut self, spill_count: usize) {
        if spill_count == 0 {
            return;
        }
        self.emit(WasmInstruction::LocalGet(self.safepoint_sp_saved_idx));
        self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
    }

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

                // (4) tag == TAG_CLOSURE: ((val >> 32) & 0xF) == TAG_CLOSURE → i32
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(0xF));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_CLOSURE as i64));
                self.emit(WasmInstruction::I64Eq);

                // (5) tag == TAG_ARRAY: ((val >> 32) & 0xF) == TAG_ARRAY → i32
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(0xF));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_ARRAY as i64));
                self.emit(WasmInstruction::I64Eq);

                // (6) tag == TAG_BOUND: ((val >> 32) & 0xF) == TAG_BOUND → i32
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(0xF));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_BOUND as i64));
                self.emit(WasmInstruction::I64Eq);

                // (7) tag == TAG_PROXY: ((val >> 32) & 0xF) == TAG_PROXY → i32
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I64Const(0xF));
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
                self.emit(WasmInstruction::I64Const(0xF));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_FUNCTION as i64));
                self.emit(WasmInstruction::I64Eq);
                self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
                self.emit(WasmInstruction::LocalGet(obj_local));
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::GlobalGet(self.function_props_base_global_idx));
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
                self.emit(WasmInstruction::I64Const(0xF));
                self.emit(WasmInstruction::I64And);
                self.emit(WasmInstruction::I64Const(value::TAG_FUNCTION as i64));
                self.emit(WasmInstruction::I64Eq);
                self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
                self.emit(WasmInstruction::LocalGet(val_local));
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::GlobalGet(self.function_props_base_global_idx));
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
                // 按 key 类型分派：数字 → $elem_get（数组元素等）；字符串 → $obj_get（命名属性）。
                self.emit_computed_get(*object, *index);
                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                Ok(false)
            }
            Instruction::SetElem {
                object,
                index,
                value,
            } => {
                // 按 key 类型分派：数字 → $elem_set；字符串 → $obj_set。
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

    /// 编译对象 spread：调用 host import obj_spread(dest, source)
    pub(crate) fn compile_object_spread(&mut self, dest: &ValueId, source: &ValueId) -> Result<()> {
        self.emit(WasmInstruction::LocalGet(self.local_idx(dest.0)));
        self.emit(WasmInstruction::LocalGet(self.local_idx(source.0)));
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::ObjSpread],
        ));
        Ok(())
    }

    /// 编译 GetSuperBase：按当前函数的 [[HomeObject]] 计算 super base。
    /// 类方法使用编译期 home metadata；对象字面量/动态 eval 通过 env.home 传入 home object。
    pub(crate) fn compile_get_super_base(&mut self, dest: &ValueId) -> Result<()> {
        match self.current_home_object {
            Some(HomeObject::Prototype(constructor_id)) => {
                let constructor = self.encode_function_ref_id(constructor_id);
                let prototype_key = self.ensure_string_ptr_const("prototype");
                self.emit(WasmInstruction::I64Const(constructor));
                self.emit(WasmInstruction::I32Const(prototype_key as i32));
                self.emit(WasmInstruction::Call(self.obj_get_func_idx));
            }
            Some(HomeObject::Constructor(constructor_id)) => {
                let constructor = self.encode_function_ref_id(constructor_id);
                self.emit(WasmInstruction::I64Const(constructor));
            }
            None => {
                self.emit(WasmInstruction::LocalGet(0));
                let home_key = self.ensure_string_ptr_const("home");
                self.emit(WasmInstruction::I32Const(home_key as i32));
                self.emit(WasmInstruction::Call(self.obj_get_func_idx));
            }
        }

        self.emit(WasmInstruction::Call(
            self.builtin_func_indices[&Builtin::ObjectGetPrototypeOf],
        ));
        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
        Ok(())
    }

    pub(crate) fn compile_get_super_constructor(&mut self, dest: &ValueId) -> Result<()> {
        if let Some(function_id) = self.current_function_id {
            let constructor = self.encode_function_ref_id(function_id);
            self.emit(WasmInstruction::I64Const(constructor));
            self.emit(WasmInstruction::Call(
                self.builtin_func_indices[&Builtin::ObjectGetPrototypeOf],
            ));
        } else {
            self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        }
        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
        Ok(())
    }
}
