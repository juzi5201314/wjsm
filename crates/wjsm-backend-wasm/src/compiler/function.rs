use anyhow::{Context, Result};
use wasm_encoder::{BlockType, Function, Instruction as WasmInstruction, MemArg, ValType};
use wjsm_ir::{Function as IrFunction, Instruction, Module as IrModule, value};

use super::state::{Cfg, CompileMode, Compiler, RegionTree};
use super::cfg_analysis::max_instruction_value_id;

impl Compiler {
    pub(crate) fn compile_function(&mut self, module: &IrModule, function: &IrFunction) -> Result<()> {
        self.current_func_returns_value = self.mode == CompileMode::Eval;
        self.ssa_local_base = if self.mode == CompileMode::Eval {
            function.params().len() as u32
        } else {
            0
        };
        // Pass 1: direct eval 函数的变量改由 shadow stack frame 承载。
        self.assign_eval_var_memory(function);
        // Pass 2: assign WASM local indices to non-eval variable names.
        self.assign_var_locals(function);

        // Pass 3: lower Phi to dedicated locals after variable locals to avoid index overlap.
        self.lower_phi_to_locals(function);

        let local_count = self.required_local_count(function);
        // scratch locals: i64 在前, i32 在后
        // string_concat (i64) at local_count
        // call_env_obj (i64) at local_count+1
        // shadow_sp (i32) at local_count+2
        // call_func_idx (i32) at local_count+3
        self.string_concat_scratch_idx = local_count;
        self.shadow_sp_scratch_idx = local_count + 2;
        self.eval_var_base_local_idx = self.shadow_sp_scratch_idx + 2;
        let param_i64_count = self.ssa_local_base;
        let total_i64_locals = local_count.saturating_sub(param_i64_count) + 2; // string_concat + call_env_obj
        let total_i32_locals = 2 + u32::from(!self.var_memory_offsets.is_empty());
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

        // 预分配函数属性对象：为每个 IR 函数调用 $obj_new(8)，将返回的 handle_idx
        // 对应 obj_table[0..num_functions-1]，存储函数属性对象的 ptr。
        // 这样后续 GetProp/SetProp 可以通过 obj_table 统一查找。
        if function.name() == "main" {
            let length_name_id = self.intern_data_string(&"length".to_string());
            let name_name_id = self.intern_data_string(&"name".to_string());
            let box_base = value::BOX_BASE as i64;
            let tag_object = (value::TAG_OBJECT << 32) as i64;
            for i in 0..self.num_ir_functions as usize {
                self.emit(WasmInstruction::I32Const(8));
                self.emit(WasmInstruction::Call(self.obj_new_func_idx));
                self.emit(WasmInstruction::LocalTee(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::I64ExtendI32U);
                self.emit(WasmInstruction::I64Const(box_base | tag_object));
                self.emit(WasmInstruction::I64Or);
                self.emit(WasmInstruction::I32Const(length_name_id as i32));
                let param_count = self.function_param_counts[i];
                self.emit(WasmInstruction::I64Const(value::encode_f64(param_count as f64)));
                self.emit(WasmInstruction::Call(self.obj_set_func_idx));
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::I64ExtendI32U);
                self.emit(WasmInstruction::I64Const(box_base | tag_object));
                self.emit(WasmInstruction::I64Or);
                self.emit(WasmInstruction::I32Const(name_name_id as i32));
                let func_name = self.function_names[i].clone();
                let name_ptr = self.intern_data_string(&func_name);
                self.emit(WasmInstruction::I64Const(value::encode_string_ptr(name_ptr)));
                self.emit(WasmInstruction::Call(self.obj_set_func_idx));
            }
            // ── 初始化 Array.prototype ──
            // 复用 shadow_sp_scratch_idx 作为 proto handle 的临时存储（proto_init_scratch）。
            // 创建 Array.prototype 对象（容量 64），存储 handle 到 Global 9
            self.emit(WasmInstruction::I32Const(64));
            self.emit(WasmInstruction::Call(self.obj_new_func_idx));
            self.emit(WasmInstruction::LocalTee(self.shadow_sp_scratch_idx));
            self.emit(WasmInstruction::GlobalSet(
                self.array_proto_handle_global_idx,
            ));
            // 为每个原型方法在 Array.prototype 上设置属性
            let method_names: [(u32, &str); 27] = [
                (0, "push"),
                (1, "pop"),
                (2, "includes"),
                (3, "indexOf"),
                (4, "join"),
                (5, "concat"),
                (6, "slice"),
                (7, "fill"),
                (8, "reverse"),
                (9, "flat"),
                (10, "shift"),
                (11, "unshift"),
                (12, "sort"),
                (13, "at"),
                (14, "copyWithin"),
                (15, "forEach"),
                (16, "map"),
                (17, "filter"),
                (18, "reduce"),
                (19, "reduceRight"),
                (20, "find"),
                (21, "findIndex"),
                (22, "some"),
                (23, "every"),
                (24, "flatMap"),
                (25, "splice"),
                (26, "isArray"),
            ];
            for (offset, name) in &method_names {
                let name_id = self.intern_data_string(name);
                let table_idx = self.arr_proto_table_base + offset;
                // 推入 boxed proto handle (i64)
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::I64ExtendI32U);
                let box_base = value::BOX_BASE as i64;
                let tag_object = (value::TAG_OBJECT << 32) as i64;
                self.emit(WasmInstruction::I64Const(box_base | tag_object));
                self.emit(WasmInstruction::I64Or);
                // 推入 name_id (i32)
                self.emit(WasmInstruction::I32Const(name_id as i32));
                // 推入编码后的函数表索引 (i64)
                self.emit(WasmInstruction::I64Const(value::encode_function_idx(
                    table_idx,
                )));
                // 调用 $obj_set(proto, name_id, func_value)
                self.emit(WasmInstruction::Call(self.obj_set_func_idx));
            }

            // ── 初始化 Object.prototype ──
            // 创建空对象（容量 64），存储 handle 到 Global 10
            self.emit(WasmInstruction::I32Const(64));
            self.emit(WasmInstruction::Call(self.obj_new_func_idx));
            self.emit(WasmInstruction::GlobalSet(
                self.object_proto_handle_global_idx,
            ));
        }

        let cfg = Cfg::from_function(function);
        let region_tree = RegionTree::build(function, &cfg)
            .map_err(|error| anyhow::anyhow!("failed to build region tree: {error:?}"))?;

        self.compiled_blocks.clear();
        self.loop_stack.clear();
        self.if_depth = 0;

        if cfg.successors.is_empty() {
            // Empty function body — emit end directly.
            self.emit(WasmInstruction::End);
        } else {
            self.compile_region_tree(module, function, &region_tree)?;
            self.emit(WasmInstruction::End);
        }

        self.codes.function(
            self.current_func
                .as_ref()
                .context("current function missing after compile")?,
        );

        // Clean up per-function state.
        self.var_locals.clear();
        self.var_memory_offsets.clear();
        self.phi_locals.clear();
        self.current_function_has_eval = false;

        Ok(())
    }

    pub(crate) fn compile_js_function(&mut self, module: &IrModule, function: &IrFunction) -> Result<()> {
        self.current_func_returns_value = true;
        // Type 12 signature: (i64 env_obj, i64 this_val, i32 args_base, i32 args_count) -> i64
        // WASM params: local 0 = env_obj (i64), local 1 = this_val (i64),
        //              local 2 = args_base_ptr (i32), local 3 = args_count (i32)
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

        // Allocate locals for declared params starting at local 4 (after env, this, args_base, args_count)
        // These will be loaded from shadow stack in the prologue
        let mut param_local_idx = 4;
        for param_name in &declared_params {
            if self.is_eval_memory_var(param_name) {
                continue;
            }
            self.var_locals
                .insert((*param_name).clone(), param_local_idx);
            param_local_idx += 1;
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

        // 计算实际需要的 local 数量
        // SSA 值从 ssa_local_base 开始分配，需要 ssa_local_base + max_ssa 个 locals
        // 但 var_locals 已经包含了声明的参数，其索引也是从 ssa_local_base 开始
        // 所以实际需要的 locals 数量 = max_ssa (SSA 值数量)
        // 而不是 ssa_local_base + max_ssa (因为 params 是 WASM 参数，不是声明的 locals)
        let max_ssa = function
            .blocks()
            .iter()
            .flat_map(|block| block.instructions())
            .map(max_instruction_value_id)
            .max()
            .map_or(0, |max| max + 1);

        // 总 local 数量
        // 为避免 SSA locals 和 var locals 索引重叠（SSA 值可能需要跨 StoreVar 保持活性，如解构），
        // 将 var locals 偏移到 SSA 最大值之后。
        let ssa_max = max_ssa + self.ssa_local_base;
        let var_rebase_start = self.ssa_local_base;
        // rebase: 所有 >= ssa_local_base 的 var/phi local 索引偏移到 ssa_max 之后
        let offset = ssa_max.saturating_sub(var_rebase_start);
        for (_name, idx) in self.var_locals.iter_mut() {
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

        // scratch locals: 所有 i64 在前，然后所有 i32（WASM locals 按 type 分组）
        // string_concat (i64) at total_locals
        // call_env_obj (i64) at total_locals+1
        // shadow_sp (i32) at total_locals+2
        // call_func_idx (i32) at total_locals+3
        self.string_concat_scratch_idx = total_locals;
        // call_env_obj scratch = string_concat + 1 (i64), computed by call_env_obj_scratch()
        self.shadow_sp_scratch_idx = total_locals + 2;
        self.eval_var_base_local_idx = self.shadow_sp_scratch_idx + 2;
        // call_func_idx = shadow_sp + 1 (i32), computed by call_func_idx_scratch()
        let total_i64_locals = total_locals.saturating_sub(4) + 2; // string_concat + call_env_obj
        let total_i32_locals = 2 + u32::from(!self.var_memory_offsets.is_empty());

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

        // ── Prologue: Load declared params from shadow stack ──
        // args_base_ptr is at local 2, args_count is at local 3
        for (i, param_name) in declared_params.iter().enumerate() {
            let param_memory_offset = self.var_memory_offsets.get(*param_name).copied();
            let param_local = self.var_locals.get(*param_name).copied();

            // if i < args_count: load from shadow stack
            // else: set to undefined
            self.emit(WasmInstruction::I32Const(i as i32)); // i
            self.emit(WasmInstruction::LocalGet(3)); // args_count
            self.emit(WasmInstruction::I32LtU); // i < args_count (unsigned)

            self.emit(WasmInstruction::If(BlockType::Empty));
            // Load from shadow stack: memory[args_base_ptr + i*8]
            self.emit(WasmInstruction::LocalGet(2)); // args_base_ptr
            self.emit(WasmInstruction::I32Const((i * 8) as i32));
            self.emit(WasmInstruction::I32Add);
            self.emit(WasmInstruction::I64Load(MemArg {
                offset: 0,
                align: 3,
                memory_index: 0,
            }));
            self.emit_store_stacked_binding(param_memory_offset, param_local);
            self.emit(WasmInstruction::Else);
            // Out of bounds: set to undefined
            self.emit(WasmInstruction::I64Const(value::encode_undefined()));
            self.emit_store_stacked_binding(param_memory_offset, param_local);
            self.emit(WasmInstruction::End);
        }

        let cfg = Cfg::from_function(function);
        let region_tree = RegionTree::build(function, &cfg)
            .map_err(|error| anyhow::anyhow!("failed to build region tree: {error:?}"))?;

        self.compiled_blocks.clear();
        self.loop_stack.clear();
        self.if_depth = 0;

        if cfg.successors.is_empty() {
            // Empty function — return undefined.
            self.emit(WasmInstruction::I64Const(value::encode_undefined()));
            self.emit(WasmInstruction::Return);
            self.emit(WasmInstruction::End);
        } else {
            self.compile_region_tree(module, function, &region_tree)?;
            self.emit(WasmInstruction::End);
        }

        self.codes.function(
            self.current_func
                .as_ref()
                .context("current function missing after compile")?,
        );

        // Clean up per-function state.
        self.var_locals.clear();
        self.var_memory_offsets.clear();
        self.phi_locals.clear();
        self.current_function_has_eval = false;

        Ok(())
    }

}
