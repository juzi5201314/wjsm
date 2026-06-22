use super::*;

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
    pub(super) fn current_spill_locals(&self) -> Vec<u32> {
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
                if is_handle && let Some(&local) = self.var_locals.get(name) {
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
    /// - key 为字符串/symbol：
    ///   - 数组 + 规范数字索引字符串（CanonicalNumericIndexString，如 "5"）→ `$elem_get`（元素）。
    ///   - 否则 → `symbol_property_key` → `$obj_get`（命名属性，含数组 .length / 原型 / 函数属性、
    ///     以及 "05"/"5.0"/"x" 等非索引字符串）。
    ///
    /// 旧实现把所有 computed key `to_int32` 后只走 `$elem_get`，导致 `a[变量]` 读 undefined、
    /// `o[字符串]` 读写错位。按 key 类型分派 + CanonicalNumericIndexString 后均正确。
    /// 索引 scratch 复用 `safepoint_sp_saved_idx`（i32）：GetElem/SetElem 非 safepoint，
    /// 其发射期间该 local 不被 spill 占用。
    pub(super) fn emit_computed_get(&mut self, object: ValueId, key: ValueId) {
        let box_base = value::BOX_BASE as i64;
        let obj_l = self.local_idx(object.0);
        let key_l = self.local_idx(key.0);
        let idx_scratch = self.safepoint_sp_saved_idx;
        let to_int32 = self.to_int32_func_idx;
        let elem_get = self.elem_get_func_idx;
        let obj_get = self.obj_get_func_idx;
        let sym_key = self.special_host_import_indices[&SpecialHostImport::SymbolPropertyKey];
        let str_arr_idx = self.special_host_import_indices[&SpecialHostImport::StringToArrayIndex];
        // is_f64(key): (key & BOX_BASE) != BOX_BASE
        self.emit(WasmInstruction::LocalGet(key_l));
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64Ne);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
        // 数字 key → elem_get（数组元素 / typedarray / 对象数字属性）。
        self.emit(WasmInstruction::LocalGet(obj_l));
        self.emit(WasmInstruction::LocalGet(key_l));
        self.emit(WasmInstruction::Call(to_int32));
        self.emit(WasmInstruction::Call(elem_get));
        self.emit(WasmInstruction::Else);
        // 字符串/symbol key：数组 + 规范数字索引 → 元素；否则命名属性。
        self.emit_is_array(obj_l);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
        self.emit(WasmInstruction::LocalGet(key_l));
        self.emit(WasmInstruction::Call(str_arr_idx));
        self.emit(WasmInstruction::LocalTee(idx_scratch));
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::I32GeS);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
        self.emit(WasmInstruction::LocalGet(obj_l));
        self.emit(WasmInstruction::LocalGet(idx_scratch));
        self.emit(WasmInstruction::Call(elem_get));
        self.emit(WasmInstruction::Else);
        self.emit_named_get(obj_l, key_l, sym_key, obj_get);
        self.emit(WasmInstruction::End);
        self.emit(WasmInstruction::Else);
        self.emit_named_get(obj_l, key_l, sym_key, obj_get);
        self.emit(WasmInstruction::End);
        self.emit(WasmInstruction::End);
    }

    /// 发射 is_array(boxed)：`(boxed >> 32) & 0xF == TAG_ARRAY` → i32 bool。
    pub(super) fn emit_is_array(&mut self, obj_l: u32) {
        self.emit(WasmInstruction::LocalGet(obj_l));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0xF));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_ARRAY as i64));
        self.emit(WasmInstruction::I64Eq);
    }

    /// 发射命名属性读取：`obj_get(obj, symbol_property_key(key))`（结果 i64 留栈上）。
    pub(super) fn emit_named_get(&mut self, obj_l: u32, key_l: u32, sym_key: u32, obj_get: u32) {
        self.emit(WasmInstruction::LocalGet(obj_l));
        self.emit(WasmInstruction::LocalGet(key_l));
        self.emit(WasmInstruction::Call(sym_key));
        self.emit(WasmInstruction::Call(obj_get));
    }

    /// 计算成员写入 `obj[key] = value` 按 key 运行期类型分派（见 `emit_computed_get`）。
    pub(super) fn emit_computed_set(&mut self, object: ValueId, key: ValueId, value: ValueId) {
        let box_base = value::BOX_BASE as i64;
        let obj_l = self.local_idx(object.0);
        let key_l = self.local_idx(key.0);
        let val_l = self.local_idx(value.0);
        let idx_scratch = self.safepoint_sp_saved_idx;
        let to_int32 = self.to_int32_func_idx;
        let elem_set = self.elem_set_func_idx;
        let obj_set = self.obj_set_func_idx;
        let sym_key = self.special_host_import_indices[&SpecialHostImport::SymbolPropertyKey];
        let str_arr_idx = self.special_host_import_indices[&SpecialHostImport::StringToArrayIndex];
        self.emit(WasmInstruction::LocalGet(key_l));
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64Ne);
        self.emit(WasmInstruction::If(BlockType::Empty));
        // 数字 key → elem_set。
        self.emit(WasmInstruction::LocalGet(obj_l));
        self.emit(WasmInstruction::LocalGet(key_l));
        self.emit(WasmInstruction::Call(to_int32));
        self.emit(WasmInstruction::LocalGet(val_l));
        self.emit(WasmInstruction::Call(elem_set));
        self.emit(WasmInstruction::Else);
        // 字符串/symbol key：数组 + 规范数字索引 → 元素写；否则命名属性写。
        self.emit_is_array(obj_l);
        self.emit(WasmInstruction::If(BlockType::Empty));
        self.emit(WasmInstruction::LocalGet(key_l));
        self.emit(WasmInstruction::Call(str_arr_idx));
        self.emit(WasmInstruction::LocalTee(idx_scratch));
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::I32GeS);
        self.emit(WasmInstruction::If(BlockType::Empty));
        self.emit(WasmInstruction::LocalGet(obj_l));
        self.emit(WasmInstruction::LocalGet(idx_scratch));
        self.emit(WasmInstruction::LocalGet(val_l));
        self.emit(WasmInstruction::Call(elem_set));
        self.emit(WasmInstruction::Else);
        self.emit_named_set(obj_l, key_l, val_l, sym_key, obj_set);
        self.emit(WasmInstruction::End);
        self.emit(WasmInstruction::Else);
        self.emit_named_set(obj_l, key_l, val_l, sym_key, obj_set);
        self.emit(WasmInstruction::End);
        self.emit(WasmInstruction::End);
    }

    /// 发射命名属性写入：`obj_set(obj, symbol_property_key(key), value)`。
    pub(super) fn emit_named_set(&mut self, obj_l: u32, key_l: u32, val_l: u32, sym_key: u32, obj_set: u32) {
        self.emit(WasmInstruction::LocalGet(obj_l));
        self.emit(WasmInstruction::LocalGet(key_l));
        self.emit(WasmInstruction::Call(sym_key));
        self.emit(WasmInstruction::LocalGet(val_l));
        self.emit(WasmInstruction::Call(obj_set));
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
    /// 总指令：2（存 base）+ 3N（写 N 值）+ 4（推进 sp）= 3N+6（vs 原 7N+2），N=35 时 111 vs 247。
    pub(super) fn emit_safepoint_spill_prologue(&mut self, spill: &[u32]) {
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
        // 一次性推进 shadow_sp = base + N*8，让 GC 扫到 spilled 值（4 条 wasm：get/add/set）
        self.emit(WasmInstruction::LocalGet(self.safepoint_sp_saved_idx));
        self.emit(WasmInstruction::I32Const((spill.len() * 8) as i32));
        self.emit(WasmInstruction::I32Add);
        self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
    }

    /// Safepoint spill epilogue：恢复 shadow_sp 到 prologue 保存的值（non-moving 无需 reload local）。
    pub(super) fn emit_safepoint_spill_epilogue(&mut self, spill_count: usize) {
        if spill_count == 0 {
            return;
        }
        self.emit(WasmInstruction::LocalGet(self.safepoint_sp_saved_idx));
        self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
    }

}
