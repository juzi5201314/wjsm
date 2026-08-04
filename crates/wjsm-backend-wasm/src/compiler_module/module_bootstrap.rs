use super::*;

/// JS 函数 prologue 种类（Step 4）。
///
/// - `Shadow`：现有 Type 12 入口 `(env, this, args_base, args_count) -> i64`，
///   形参从 shadow stack 读取（argc 检查 + 越界补 undefined）。
/// - `Direct(n)`：fast 入口 `(env, this, p0..p{n-1}) -> i64`，形参即 wasm 参数
///   （寄存器直传），无 args_base/args_count、无 argc 分支、无 shadow 读。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrologueKind {
    Shadow,
    Direct(u32),
}

impl PrologueKind {
    /// wasm 参数个数（前几个 local 由类型签名提供，不入 Function::new locals）。
    fn wasm_param_count(self) -> u32 {
        match self {
            PrologueKind::Shadow => 4,
            PrologueKind::Direct(n) => 2 + n,
        }
    }
}

impl Compiler {
    fn emit_startup_phase_call(&mut self, func_idx: u32) {
        self.emit(WasmInstruction::Call(func_idx));
        self.emit(WasmInstruction::LocalSet(self.string_concat_scratch_idx));
        self.emit(WasmInstruction::LocalGet(self.string_concat_scratch_idx));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::I32Const(value::TAG_EXCEPTION as i32));
        self.emit(WasmInstruction::I32Eq);
        self.emit(WasmInstruction::If(BlockType::Empty));
        self.emit(WasmInstruction::LocalGet(self.string_concat_scratch_idx));
        self.emit_eval_var_frame_exit();
        self.emit(WasmInstruction::Return);
        self.emit(WasmInstruction::End);
    }

    pub(super) fn compile_bootstrap_once_function(&mut self) {
        let previous_shadow_sp_scratch_idx = self.shadow_sp_scratch_idx;
        self.shadow_sp_scratch_idx = 0;
        self.current_func = Some(Function::new(vec![(1, ValType::I32)]));

        self.emit(WasmInstruction::GlobalGet(self.bootstrap_done_global_idx));
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::I32Ne);
        self.emit(WasmInstruction::If(BlockType::Empty));
        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        self.emit(WasmInstruction::Return);
        self.emit(WasmInstruction::End);
        // P2.2: globals 初始化已移到 __wjsm_init_globals 函数中，
        // 由 runtime 在 initialize_host_post_bootstrap 之前调用。
        // ── 初始化 Object.prototype ──
        // 必须先于 Array.prototype：obj_new 会把新对象的 [[Prototype]] header 写成
        // 当前 G_OBJECT_PROTO_HANDLE。若 Array.prototype 先分配，则其原型链断在 null，
        // 导致 [] instanceof Object 为 false、getPrototypeOf(Array.prototype) 为 null。
        self.emit(WasmInstruction::GlobalGet(
            self.object_proto_handle_global_idx,
        ));
        self.emit(WasmInstruction::I32Const(-1));
        self.emit(WasmInstruction::I32Eq);
        self.emit(WasmInstruction::If(BlockType::Empty));
        self.emit(WasmInstruction::I32Const(64));
        self.emit(WasmInstruction::Call(self.obj_new_func_idx));
        self.emit(WasmInstruction::LocalTee(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::GlobalSet(
            self.object_proto_handle_global_idx,
        ));
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::I64ExtendI32U);
        let object_tag = value::BOX_BASE as i64 | ((value::TAG_OBJECT << 32) as i64);
        self.emit(WasmInstruction::I64Const(object_tag));
        self.emit(WasmInstruction::I64Or);
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::ObjectProtoInit],
        ));
        self.emit(WasmInstruction::Drop);
        self.emit(WasmInstruction::End);

        // ── 初始化 Array.prototype ──
        // 此时 Object.prototype 已就绪，obj_new 写入的 [[Prototype]] header 即为
        // Object.prototype，建立 Array.prototype → Object.prototype 原型链。
        self.emit(WasmInstruction::I32Const(64));
        self.emit(WasmInstruction::Call(self.obj_new_func_idx));
        self.emit(WasmInstruction::LocalTee(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::GlobalSet(
            self.array_proto_handle_global_idx,
        ));
        for (offset, (_, spec)) in array_proto_method_specs().enumerate() {
            let name = array_proto_property_name(spec.name).expect("array prototype import name");
            let name_id = self.intern_data_string(&name);
            let table_idx = self.arr_proto_table_base + offset as u32;
            self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
            self.emit(WasmInstruction::I64ExtendI32U);
            let box_base = value::BOX_BASE as i64;
            let tag_object = (value::TAG_OBJECT << 32) as i64;
            self.emit(WasmInstruction::I64Const(box_base | tag_object));
            self.emit(WasmInstruction::I64Or);
            self.emit(WasmInstruction::I32Const(name_id as i32));
            self.emit(WasmInstruction::I64Const(value::encode_function_idx(
                table_idx,
            )));
            self.emit(WasmInstruction::Call(self.obj_set_func_idx));
        }

        self.emit(WasmInstruction::GlobalGet(self.obj_table_count_global_idx));
        self.emit(WasmInstruction::GlobalSet(
            self.function_props_base_global_idx,
        ));
        self.emit(WasmInstruction::I32Const(1));
        self.emit(WasmInstruction::GlobalSet(self.bootstrap_done_global_idx));
        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        self.emit(WasmInstruction::End);

        self.codes.function(
            self.current_func
                .as_ref()
                .expect("bootstrap function should be initialized"),
        );
        self.current_func = None;
        self.shadow_sp_scratch_idx = previous_shadow_sp_scratch_idx;
    }

    pub(super) fn compile_init_function_props_function(&mut self) {
        let previous_shadow_sp_scratch_idx = self.shadow_sp_scratch_idx;
        self.shadow_sp_scratch_idx = 0;
        self.current_func = Some(Function::new(vec![(2, ValType::I32)]));

        if self.mode == CompileMode::Normal {
            self.emit(WasmInstruction::GlobalGet(
                self.function_props_done_global_idx,
            ));
            self.emit(WasmInstruction::I32Const(0));
            self.emit(WasmInstruction::I32Ne);
            self.emit(WasmInstruction::If(BlockType::Empty));
            self.emit(WasmInstruction::I64Const(value::encode_undefined()));
            self.emit(WasmInstruction::Return);
            self.emit(WasmInstruction::End);

            // cold startup: obj_table_count == function_props_base，按旧布局从 base 分配；
            // snapshot restore: obj_table_count 已在 restored heap 之后，不能回退覆盖 snapshot 对象。
            // 此时把当前模块的 function_props_base 前移到 obj_table_count，再连续分配本模块函数属性对象。
            self.emit(WasmInstruction::GlobalGet(self.obj_table_count_global_idx));
            self.emit(WasmInstruction::GlobalGet(
                self.function_props_base_global_idx,
            ));
            self.emit(WasmInstruction::I32GtU);
            self.emit(WasmInstruction::If(BlockType::Empty));
            self.emit(WasmInstruction::GlobalGet(self.obj_table_count_global_idx));
            self.emit(WasmInstruction::GlobalSet(
                self.function_props_base_global_idx,
            ));
            self.emit(WasmInstruction::Else);
            self.emit(WasmInstruction::GlobalGet(
                self.function_props_base_global_idx,
            ));
            self.emit(WasmInstruction::GlobalSet(self.obj_table_count_global_idx));
            self.emit(WasmInstruction::End);
        }

        let length_name_id = self.intern_data_string("length");
        let name_name_id = self.intern_data_string("name");
        let constructor_name_id = self.intern_data_string("constructor");
        let prototype_name_id = self.intern_data_string("prototype");
        let box_base = value::BOX_BASE as i64;
        let tag_object = (value::TAG_OBJECT << 32) as i64;
        let proto_handle_local = 1u32;
        // Pass 1: 为所有 IR 函数分配连续的属性对象（function_props_base + i）
        // 必须连续分配，因为 obj_get/obj_set 假设 handle = func_idx + function_props_base
        for i in 0..self.num_ir_functions as usize {
            self.emit(WasmInstruction::I32Const(8));
            self.emit(WasmInstruction::Call(self.obj_new_func_idx));
            self.emit(WasmInstruction::LocalTee(self.shadow_sp_scratch_idx));
            self.emit(WasmInstruction::I64ExtendI32U);
            self.emit(WasmInstruction::I64Const(box_base | tag_object));
            self.emit(WasmInstruction::I64Or);
            self.emit(WasmInstruction::I32Const(length_name_id as i32));
            let param_count = self.function_param_counts[i];
            self.emit(WasmInstruction::I64Const(value::encode_f64(
                param_count as f64,
            )));
            self.emit(WasmInstruction::Call(self.obj_set_func_idx));
            self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
            self.emit(WasmInstruction::I64ExtendI32U);
            self.emit(WasmInstruction::I64Const(box_base | tag_object));
            self.emit(WasmInstruction::I64Or);
            self.emit(WasmInstruction::I32Const(name_name_id as i32));
            let func_name = self.function_names[i].clone();
            let name_ptr = self.intern_data_string(&func_name);
            self.emit(WasmInstruction::I64Const(value::encode_string_ptr(
                name_ptr,
            )));
            self.emit(WasmInstruction::Call(self.obj_set_func_idx));
        }

        // Pass 2: 为需要 prototype 的函数创建 prototype 对象并设置 constructor 属性
        // 此时所有 function_props 已分配完毕，prototype 对象从 function_props_base + num_ir_functions 开始
        for i in 0..self.num_ir_functions as usize {
            if !self.function_needs_prototype[i] {
                continue;
            }
            // func_props handle = function_props_base + i（Pass 1 已分配）
            self.emit(WasmInstruction::I32Const(i as i32));
            self.emit(WasmInstruction::GlobalGet(
                self.function_props_base_global_idx,
            ));
            self.emit(WasmInstruction::I32Add);
            self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));

            // 1. 创建 prototype 对象
            self.emit(WasmInstruction::I32Const(4));
            self.emit(WasmInstruction::Call(self.obj_new_func_idx));
            self.emit(WasmInstruction::LocalSet(proto_handle_local));

            // 2. prototype.constructor = encode_function_idx(i)
            self.emit(WasmInstruction::LocalGet(proto_handle_local));
            self.emit(WasmInstruction::I64ExtendI32U);
            self.emit(WasmInstruction::I64Const(box_base | tag_object));
            self.emit(WasmInstruction::I64Or);
            self.emit(WasmInstruction::I32Const(constructor_name_id as i32));
            self.emit(WasmInstruction::I64Const(value::encode_function_idx(
                i as u32,
            )));
            self.emit(WasmInstruction::Call(self.obj_set_func_idx));

            // 3. func_props.prototype = proto 对象
            self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
            self.emit(WasmInstruction::I64ExtendI32U);
            self.emit(WasmInstruction::I64Const(box_base | tag_object));
            self.emit(WasmInstruction::I64Or);
            self.emit(WasmInstruction::I32Const(prototype_name_id as i32));
            self.emit(WasmInstruction::LocalGet(proto_handle_local));
            self.emit(WasmInstruction::I64ExtendI32U);
            self.emit(WasmInstruction::I64Const(box_base | tag_object));
            self.emit(WasmInstruction::I64Or);
            self.emit(WasmInstruction::Call(self.obj_set_func_idx));
        }

        if self.mode == CompileMode::Normal {
            self.emit(WasmInstruction::I32Const(1));
            self.emit(WasmInstruction::GlobalSet(
                self.function_props_done_global_idx,
            ));
        }
        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        self.emit(WasmInstruction::End);

        self.codes.function(
            self.current_func
                .as_ref()
                .expect("function props initializer should be initialized"),
        );
        self.current_func = None;
        self.shadow_sp_scratch_idx = previous_shadow_sp_scratch_idx;
    }

    /// 为当前函数的动态 Call/ConstructCall 调用点分配独立缓存 locals。
    ///
    /// Wasm locals 按参数、i64 声明、i32 声明分组；cache 的 i64 段接在既有
    /// i64 scratch 后，func_idx 的 i32 段接在全部既有 i32 scratch 后，避免
    /// shadow_sp/call_func/safepoint/computed/eval locals 发生类型或索引碰撞。
    fn setup_call_cache_locals(
        &mut self,
        function: &IrFunction,
        total_locals: u32,
        param_count_for_i64: u32,
        existing_i32_scratch: u32,
    ) -> (u32, u32) {
        self.current_call_cache_slots.clear();
        let cache_keys: Vec<(usize, usize)> = function
            .blocks()
            .iter()
            .enumerate()
            .flat_map(|(block_idx, block)| {
                block.instructions().iter().enumerate().filter_map(
                    move |(instr_idx, instruction)| {
                        matches!(
                            instruction,
                            Instruction::Call { .. } | Instruction::ConstructCall { .. }
                        )
                        .then_some((block_idx, instr_idx))
                    },
                )
            })
            .collect();
        let cache_count = cache_keys.len() as u32;
        let cache_i64_base = total_locals + 2;
        let cache_i32_base = cache_i64_base + 2 * cache_count;
        let cache_func_i32_base = cache_i32_base + existing_i32_scratch;
        for (slot, key) in cache_keys.into_iter().enumerate() {
            let i64_base = cache_i64_base + 2 * slot as u32;
            self.current_call_cache_slots.insert(
                key,
                (i64_base, i64_base + 1, cache_func_i32_base + slot as u32),
            );
        }

        // 闭包表只追加不可变；缓存 locals 不登记为 GC root，但调用点的 callee
        // 仍由当前指令 liveness spill 保活，故 miss/hit 都不会改变 GC 语义。
        self.string_concat_scratch_idx = total_locals;
        self.shadow_sp_scratch_idx = cache_i32_base;
        self.safepoint_sp_saved_idx = cache_i32_base + 2;
        self.computed_idx_scratch_idx = cache_i32_base + 3;
        self.eval_var_base_local_idx = cache_i32_base + 4;

        let total_i64_locals =
            total_locals.saturating_sub(param_count_for_i64) + 2 + 2 * cache_count;
        let total_i32_locals = existing_i32_scratch + cache_count;
        (total_i64_locals, total_i32_locals)
    }

    pub(crate) fn compile_function(
        &mut self,
        module: &IrModule,
        function: &IrFunction,
        function_id: wjsm_ir::FunctionId,
    ) -> Result<()> {
        self.current_func_is_main = is_module_entry_ir_function(function.name());
        self.current_func_returns_value =
            self.mode == CompileMode::Eval || self.current_func_is_main;
        // 与 compile_js_function 对齐：模块入口也需要 current_function_id，
        // 否则 GcAnalysis::direct_call_target 恒不命中，入口对函数声明的调用
        // 会全部退化为通用调用路径（tag 分派 + call_indirect）。
        self.current_home_object = function.home_object;
        self.current_function_id = Some(function_id);
        self.const_string_ptrs.clear();
        self.current_call_cache_slots.clear();
        self.ssa_local_base = if self.mode == CompileMode::Eval {
            function.params().len() as u32
        } else {
            0
        };
        self.begin_function_debug(function.name());
        // Pass 1: direct eval 函数的变量改由 shadow stack frame 承载。
        self.assign_eval_var_memory(function);
        // Pass 2: assign WASM local indices to non-eval variable names.
        self.assign_var_locals(function);

        // Pass 3: lower Phi to dedicated locals after variable locals to avoid index overlap.
        self.lower_phi_to_locals(function);

        // ── GC safepoint（P2）：计算 liveness + ValueTy（per-ValueId + 变量）──
        // 供 NewObject/NewArray/Call/CallBuiltin/SuperCall/ConstructCall 的 safepoint spill。
        self.setup_gc_safepoint_analysis(module, function);
        self.current_emit_block_idx = 0;
        self.current_emit_instr_idx = 0;

        let local_count = self.required_local_count(function);
        // string_concat (i64) at local_count
        // call_env_obj (i64) at local_count+1
        // shadow_sp (i32) at local_count+2
        // call_func_idx (i32) at local_count+3
        // safepoint_sp_saved (i32) at local_count+4  (P2: safepoint spill save/restore)
        // computed_idx_scratch (i32) at local_count+5  (emit_computed_get/set 暂存规范数字索引)
        self.string_concat_scratch_idx = local_count;
        self.shadow_sp_scratch_idx = local_count + 2;
        self.safepoint_sp_saved_idx = local_count + 4;
        self.computed_idx_scratch_idx = local_count + 5;
        self.eval_var_base_local_idx = local_count + 6;
        let param_i64_count = self.ssa_local_base;
        let total_i64_locals = local_count.saturating_sub(param_i64_count) + 2;
        let total_i32_locals = 4 + u32::from(!self.var_memory_offsets.is_empty());
        let locals = if total_i64_locals == 0 && total_i32_locals == 0 {
            Vec::new()
        } else {
            vec![
                (total_i64_locals, ValType::I64),
                (total_i32_locals, ValType::I32),
            ]
        };
        self.current_func = Some(Function::new(locals));
        self.emit_eval_var_frame_enter();

        // ── GC safepoint 容量检查（P2 T2.3，spec IMPL-13/R2）──
        // 防 spill 区溢出覆盖对象堆：shadow_sp + frame_size + spill_upper_bound <= shadow_stack_end。
        // spill_upper_bound = 本函数所有 safepoint 的最大 live handle local 数 × 8。
        self.emit_safepoint_capacity_check(module, function);

        if is_module_entry_ir_function(function.name()) && self.mode == CompileMode::Normal {
            // P2.2: globals 初始化在 __wjsm_bootstrap_once 中执行（bootstrap_done 检查之后），
            // 确保 run_bootstrap_only 直接调用 bootstrap 时也能正确初始化 globals。
            self.emit_startup_phase_call(self.bootstrap_func_idx);
            self.emit_startup_phase_call(self.init_function_props_func_idx);
        }

        let start_idx = function.entry().0 as usize;
        if !function.blocks().is_empty() && start_idx >= function.blocks().len() {
            anyhow::bail!(
                "function '{}' entry block {} is out of range ({} blocks)",
                function.name(),
                start_idx,
                function.blocks().len()
            );
        }

        self.compiled_blocks.clear();
        self.loop_stack.clear();
        self.if_depth = 0;

        if function.blocks().is_empty() {
            // Empty function body — emit end directly.
            self.emit(WasmInstruction::End);
        } else {
            self.compile_control_flow(module, function, start_idx)?;
            self.emit(WasmInstruction::End);
        }

        self.codes.function(
            self.current_func
                .as_ref()
                .context("current function missing after compile")?,
        );

        // 在清空 var_locals 前收集调试局部变量映射。
        self.collect_debug_locals();

        // Clean up per-function state.
        self.var_locals.clear();
        self.var_memory_offsets.clear();
        self.phi_locals.clear();
        self.current_function_has_eval = false;
        self.current_fn_liveness = None;
        self.current_fn_value_ty = None;
        self.current_fn_var_liveness = None;
        self.current_fn_var_ty = None;
        self.current_home_object = None;
        self.current_function_id = None;

        Ok(())
    }

    pub(crate) fn compile_js_function(
        &mut self,
        module: &IrModule,
        function: &IrFunction,
        function_id: wjsm_ir::FunctionId,
        prologue_kind: PrologueKind,
    ) -> Result<()> {
        self.current_func_returns_value = true;
        self.current_home_object = function.home_object;
        self.current_function_id = Some(function_id);
        self.begin_function_debug(function.name());
        self.const_string_ptrs.clear();
        self.current_call_cache_slots.clear();
        // WASM params（Shadow）: local 0 = env_obj (i64), local 1 = this_val (i64),
        //              local 2 = args_base_ptr (i32), local 3 = args_count (i32)
        // WASM params（Direct）: local 0 = env (i64), local 1 = this (i64),
        //              local 2+i = 第 i 个声明形参 (i64)
        self.assign_eval_var_memory(function);

        // Map $env/$this to WASM params (both bare and scoped names)
        self.var_locals.clear();
        self.var_locals.insert("$env".to_string(), 0);
        self.var_locals.insert("$this".to_string(), 1);

        // Count declared params (excluding $env/$this in both bare and scoped forms)
        let declared_params: Vec<&String> = function
            .params()
            .iter()
            .filter(|p| {
                let s = p.as_str();
                s != "$env" && s != "$this" && !s.ends_with(".$env") && !s.ends_with(".$this")
            })
            .collect();

        // 形参 local 分配：
        // - Shadow：local 4 起（env/this/args_base/args_count 之后），prologue 从 shadow 读。
        // - Direct(n)：形参即 wasm 参数 local 2+i（env/this 之后），寄存器直传。
        let mut param_local_idx = match prologue_kind {
            PrologueKind::Shadow => 4,
            PrologueKind::Direct(n) => 2 + n,
        };
        match prologue_kind {
            PrologueKind::Shadow => {
                for param_name in &declared_params {
                    if self.is_eval_memory_var(param_name) {
                        continue;
                    }
                    self.var_locals
                        .insert((*param_name).clone(), param_local_idx);
                    param_local_idx += 1;
                }
            }
            PrologueKind::Direct(_) => {
                for (i, param_name) in declared_params.iter().enumerate() {
                    self.var_locals
                        .insert((*param_name).clone(), 2 + i as u32);
                }
            }
        }
        // Map scoped $env/$this param names to the same locals as bare names
        for p in function.params() {
            if p.ends_with(".$env") {
                self.var_locals.insert(p.clone(), 0);
            } else if p.ends_with(".$this") {
                self.var_locals.insert(p.clone(), 1);
            }
        }
        self.ssa_local_base = param_local_idx;
        // Variable locals start after param locals
        self.next_var_local = param_local_idx;
        // Assign variable locals for LoadVar/StoreVar.
        for block in function.blocks() {
            for instruction in block.instructions() {
                let name = match instruction {
                    Instruction::LoadVar { name, .. } | Instruction::StoreVar { name, .. } => name,
                    _ => continue,
                };
                if self.is_eval_memory_var(name) {
                    continue;
                }
                self.var_locals.entry(name.clone()).or_insert_with(|| {
                    let idx = self.next_var_local;
                    self.next_var_local += 1;
                    idx
                });
            }
        }
        self.lower_phi_to_locals(function);

        // ── GC safepoint（P2）：计算 liveness + ValueTy（per-ValueId + 变量）──
        self.setup_gc_safepoint_analysis(module, function);
        self.current_emit_block_idx = 0;
        self.current_emit_instr_idx = 0;

        let max_ssa = function
            .blocks()
            .iter()
            .flat_map(|block| block.instructions())
            .map(max_instruction_value_id)
            .max()
            .map_or(0, |max| max + 1);
        let ssa_max = max_ssa + self.ssa_local_base;
        let var_rebase_start = self.ssa_local_base;
        let offset = ssa_max.saturating_sub(var_rebase_start);
        for idx in self.var_locals.values_mut() {
            if *idx >= var_rebase_start {
                *idx += offset;
            }
        }
        let total_var_locals = self.next_var_local + offset;
        for idx in self.phi_locals.values_mut() {
            if *idx >= var_rebase_start {
                *idx += offset;
            }
        }
        let total_locals = ssa_max
            .max(total_var_locals)
            .max(self.phi_locals.values().copied().max().map_or(0, |m| m + 1));
        let existing_i32_scratch = 4 + u32::from(!self.var_memory_offsets.is_empty());
        let (total_i64_locals, total_i32_locals) = self.setup_call_cache_locals(
            function,
            total_locals,
            prologue_kind.wasm_param_count(),
            existing_i32_scratch,
        );
        let locals = if total_i64_locals == 0 && total_i32_locals == 0 {
            Vec::new()
        } else {
            vec![
                (total_i64_locals, ValType::I64),
                (total_i32_locals, ValType::I32),
            ]
        };
        self.current_func = Some(Function::new(locals));
        self.emit_eval_var_frame_enter();

        // ── GC safepoint 容量检查（P2 T2.3，spec IMPL-13/R2）──
        self.emit_safepoint_capacity_check(module, function);

        // ── Prologue: Load declared params from shadow stack（仅 Shadow 入口）──
        // args_base_ptr is at local 2, args_count is at local 3
        // Direct 入口的形参即 wasm 参数（寄存器直传），无 shadow 读、无 argc 分支。
        if prologue_kind == PrologueKind::Shadow {
            for (i, param_name) in declared_params.iter().enumerate() {
                let param_memory_offset = self.var_memory_offsets.get(*param_name).copied();
                let param_local = self.var_locals.get(*param_name).copied();

                // if i < args_count: load from shadow stack
                // else: set to undefined
                self.emit(WasmInstruction::I32Const(i as i32)); // i
                self.emit(WasmInstruction::LocalGet(3)); // args_count
                self.emit(WasmInstruction::I32LtU); // i < args_count (unsigned)

                self.emit(WasmInstruction::If(BlockType::Empty));
                // Load from shadow stack: shadow_memory[args_base_ptr + i*8]
                self.emit(WasmInstruction::LocalGet(2)); // args_base_ptr
                self.emit(WasmInstruction::I32Const((i * 8) as i32));
                self.emit(WasmInstruction::I32Add);
                self.emit(WasmInstruction::I64Load(crate::shadow_mem_arg(0)));
                self.emit_store_stacked_binding(param_memory_offset, param_local);
                self.emit(WasmInstruction::Else);
                // Out of bounds: set to undefined
                self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                self.emit_store_stacked_binding(param_memory_offset, param_local);
                self.emit(WasmInstruction::End);
            }
        }

        self.emit_init_module_global_for_js_function(function);
        let start_idx = function.entry().0 as usize;
        if !function.blocks().is_empty() && start_idx >= function.blocks().len() {
            anyhow::bail!(
                "function '{}' entry block {} is out of range ({} blocks)",
                function.name(),
                start_idx,
                function.blocks().len()
            );
        }

        self.compiled_blocks.clear();
        self.loop_stack.clear();
        self.if_depth = 0;

        if function.blocks().is_empty() {
            // Empty function — return undefined.
            self.emit(WasmInstruction::I64Const(value::encode_undefined()));
            self.emit(WasmInstruction::Return);
            self.emit(WasmInstruction::End);
        } else {
            self.compile_control_flow(module, function, start_idx)?;
            self.emit(WasmInstruction::End);
        }

        self.codes.function(
            self.current_func
                .as_ref()
                .context("current function missing after compile")?,
        );

        // 在清空 var_locals 前收集调试局部变量映射。
        self.collect_debug_locals();

        // Clean up per-function state.
        self.var_locals.clear();
        self.var_memory_offsets.clear();
        self.phi_locals.clear();
        self.current_function_has_eval = false;
        self.current_home_object = None;
        self.current_function_id = None;
        self.current_fn_liveness = None;
        self.current_fn_value_ty = None;
        self.current_fn_var_liveness = None;
        self.current_fn_var_ty = None;

        Ok(())
    }
}
