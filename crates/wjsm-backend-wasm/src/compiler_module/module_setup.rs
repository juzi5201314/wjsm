use super::*;

impl Compiler {
    pub(crate) fn local_idx(&self, val_id: u32) -> u32 {
        val_id + self.ssa_local_base
    }

    /// call_func_idx scratch local (i32) — 存放解析后的函数表索引
    pub(crate) fn call_func_idx_scratch(&self) -> u32 {
        self.shadow_sp_scratch_idx + 1
    }

    /// GC safepoint 容量检查（P2 T2.3，spec IMPL-13/R2）。
    /// 函数 prologue 一次性检查：当前 shadow_sp + 本函数 spill_upper_bound
    /// 是否超出 shadow_stack_end。若超出，trap（防止 spill 区溢出覆盖对象堆）。
    ///
    /// spill_upper_bound = 本函数所有 safepoint 处 live handle local 数的最大值 × 8。
    /// 编译期静态计算；运行期只发一个比较。
    pub(super) fn emit_safepoint_capacity_check(
        &mut self,
        _module: &IrModule,
        function: &IrFunction,
    ) {
        let spill_upper_bound = self.compute_max_spill_bytes(function);
        if spill_upper_bound == 0 {
            return;
        }
        // if (shadow_sp + spill_upper_bound) > shadow_stack_end:
        // 走统一 shadow-stack overflow host import，写入可诊断 runtime_error 后 trap。
        self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
        self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));
        self.emit_shadow_stack_overflow_check(spill_upper_bound as i32);
    }

    /// 计算本函数所有 safepoint 处 live handle local 数的最大值 × 8（字节）。
    pub(super) fn compute_max_spill_bytes(&self, function: &IrFunction) -> usize {
        let Some(liveness) = &self.current_fn_liveness else {
            return 0;
        };
        let value_ty = self.current_fn_value_ty.as_ref();
        let var_liveness = self.current_fn_var_liveness.as_ref();
        let var_ty = self.current_fn_var_ty.as_ref();
        let mut max = 0usize;
        for (bid, instr_map) in liveness {
            let block = match function.block_by_id(*bid) {
                Some(b) => b,
                None => continue,
            };
            let instrs = block.instructions();
            for (i, ins) in instrs.iter().enumerate() {
                if !Self::is_safepoint(ins) {
                    continue;
                }
                let mut cnt = 0usize;
                if let Some(live) = instr_map.get(&i) {
                    cnt += live
                        .iter()
                        .filter(|v| {
                            value_ty
                                .and_then(|m| m.get(v))
                                .is_none_or(|t| *t == ValueTy::Handle)
                        })
                        .count();
                }
                // 变量 spill 上界：与 current_spill_locals 一致——存活且可能持有 handle 的变量 local。
                // 变量 local 与 SSA 值 local 索引不相交，故直接相加即精确上界。
                if let Some(names) = var_liveness
                    .and_then(|m| m.get(bid))
                    .and_then(|m| m.get(&i))
                {
                    cnt += names
                        .iter()
                        .filter(|name| {
                            self.var_locals.contains_key(*name)
                                && var_ty
                                    .and_then(|m| m.get(*name))
                                    .is_none_or(|t| *t == ValueTy::Handle)
                        })
                        .count();
                }
                max = max.max(cnt);
            }
        }
        max * 8
    }

    /// 计算并缓存当前函数的 GC safepoint 分析：per-ValueId liveness + 变量 liveness +
    /// 两者的 ValueTy。compile_function / compile_eval 入口各调用一次。
    pub(super) fn setup_gc_safepoint_analysis(&mut self, module: &IrModule, function: &IrFunction) {
        // per-ValueId liveness（扁平 → 嵌套便于查询）。
        let flat = crate::analysis_liveness::compute_liveness(function);
        let mut nested: HashMap<
            wjsm_ir::BasicBlockId,
            HashMap<usize, std::collections::HashSet<wjsm_ir::ValueId>>,
        > = HashMap::new();
        for ((bid, i), set) in flat {
            nested.entry(bid).or_default().insert(i, set);
        }
        self.current_fn_liveness = Some(nested);

        // 变量 liveness（弥补 per-ValueId liveness 看不到变量存活的空洞，供变量 spill）。
        let var_flat = crate::analysis_liveness::compute_var_liveness(function);
        let mut var_nested: HashMap<
            wjsm_ir::BasicBlockId,
            HashMap<usize, std::collections::HashSet<String>>,
        > = HashMap::new();
        for ((bid, i), set) in var_flat {
            var_nested.entry(bid).or_default().insert(i, set);
        }
        self.current_fn_var_liveness = Some(var_nested);

        let (value_ty, var_ty) = crate::analysis_value_ty::infer_value_and_var_ty(module, function);
        self.current_fn_value_ty = Some(value_ty);
        self.current_fn_var_ty = Some(var_ty);
    }

    /// call_env_obj scratch local (i64) — 存放解析后的闭包环境对象
    pub(crate) fn call_env_obj_scratch(&self) -> u32 {
        self.string_concat_scratch_idx + 1
    }
    /// Nested JS functions may LoadVar `$0.$global` (builtin globals like `$262`); only `main` stores it at init.
    pub(super) fn emit_init_module_global_for_js_function(&mut self, function: &IrFunction) {
        let needs = function
            .blocks()
            .iter()
            .flat_map(|b| b.instructions())
            .any(|inst| {
                matches!(
                    inst,
                    Instruction::LoadVar { name, .. } | Instruction::StoreVar { name, .. }
                        if name == "$0.$global"
                )
            });
        if !needs {
            return;
        }
        let Some(&local_idx) = self.var_locals.get("$0.$global") else {
            return;
        };
        let func_idx = self
            .builtin_func_indices
            .get(&Builtin::CreateGlobalObject)
            .copied()
            .expect("create_global_object builtin");
        self.emit(WasmInstruction::Call(func_idx));
        self.emit(WasmInstruction::LocalSet(local_idx));
    }

    pub(crate) fn emit_resolve_callable_for_helper(
        &self,
        func: &mut Function,
        callee_local: u32,
        func_idx_local: u32,
        env_obj_local: u32,
    ) {
        func.instruction(&WasmInstruction::LocalGet(callee_local));
        func.instruction(&WasmInstruction::I64Const(32));
        func.instruction(&WasmInstruction::I64ShrU);
        func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
        func.instruction(&WasmInstruction::I64And);
        func.instruction(&WasmInstruction::I64Const(value::TAG_CLOSURE as i64));
        func.instruction(&WasmInstruction::I64Eq);
        func.instruction(&WasmInstruction::If(BlockType::Empty));

        func.instruction(&WasmInstruction::LocalGet(callee_local));
        func.instruction(&WasmInstruction::I32WrapI64);
        func.instruction(&WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::ClosureGetFunc],
        ));
        func.instruction(&WasmInstruction::LocalSet(func_idx_local));
        func.instruction(&WasmInstruction::LocalGet(callee_local));
        func.instruction(&WasmInstruction::I32WrapI64);
        func.instruction(&WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::ClosureGetEnv],
        ));
        func.instruction(&WasmInstruction::LocalSet(env_obj_local));

        func.instruction(&WasmInstruction::Else);
        func.instruction(&WasmInstruction::LocalGet(callee_local));
        func.instruction(&WasmInstruction::I32WrapI64);
        func.instruction(&WasmInstruction::LocalSet(func_idx_local));
        func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
        func.instruction(&WasmInstruction::LocalSet(env_obj_local));
        func.instruction(&WasmInstruction::End);
    }
}
