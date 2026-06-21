use super::*;
use crate::host_import_registry::{
    SpecialHostImport, array_proto_method_specs, array_proto_property_name, array_proto_table_hash,
    array_proto_table_len, host_import_specs,
};

impl Compiler {
    /// Convert an IR ValueId to a WASM local index, accounting for ssa_local_base.
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
    fn emit_safepoint_capacity_check(&mut self, _module: &IrModule, function: &IrFunction) {
        let spill_upper_bound = self.compute_max_spill_bytes(function);
        if spill_upper_bound == 0 {
            return;
        }
        // if (shadow_sp + spill_upper_bound) > shadow_stack_end: unreachable
        self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
        self.emit(WasmInstruction::I32Const(spill_upper_bound as i32));
        self.emit(WasmInstruction::I32Add);
        self.emit(WasmInstruction::GlobalGet(self.shadow_stack_end_global_idx));
        self.emit(WasmInstruction::I32GtU);
        self.emit(WasmInstruction::If(BlockType::Empty));
        self.emit(WasmInstruction::Unreachable);
        self.emit(WasmInstruction::End);
    }

    /// 计算本函数所有 safepoint 处 live handle local 数的最大值 × 8（字节）。
    fn compute_max_spill_bytes(&self, function: &IrFunction) -> usize {
        let Some(ref liveness) = self.current_fn_liveness else {
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
    fn setup_gc_safepoint_analysis(&mut self, module: &IrModule, function: &IrFunction) {
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
    fn emit_init_module_global_for_js_function(&mut self, function: &IrFunction) {
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

    pub(crate) fn compile_module(&mut self, module: &IrModule) -> Result<()> {
        // Pass 0: 模块级 GC 分析（Layer 3c）
        self.gc_analysis = Some(GcAnalysis::analyze(module));

        // Pass 1: Register all IR functions as WASM functions.
        let mut main_wasm_idx: Option<u32> = None;
        for (i, function) in module.functions().iter().enumerate() {
            let wasm_idx = self._next_import_func;
            self.function_name_to_wasm_idx
                .insert(function.name().to_string(), wasm_idx);

            let declared_param_count = function
                .params()
                .iter()
                .filter(|p| {
                    let s = p.as_str();
                    s != "$env" && s != "$this" && !s.ends_with(".$env") && !s.ends_with(".$this")
                })
                .count() as u32;
            self.function_param_counts.push(declared_param_count);
            self.function_names.push(function.name().to_string());

            if is_module_entry_ir_function(function.name()) {
                if self.mode == CompileMode::Eval {
                    // eval entry: Type 3 = (scope_env: i64) -> i64 completion value
                    self.functions.function(3);
                } else {
                    // main: Type 4 = () -> i64 (返回异常值或 undefined)
                    self.functions.function(4);
                }
                main_wasm_idx = Some(wasm_idx);
            } else {
                // JS functions: Type 12 = (i64, i64, i32, i32) -> i64 (含 env_obj)
                self.functions.function(12);
            }

            self.push_func_table(wasm_idx);
            self.function_id_to_wasm_idx.insert(i as u32, wasm_idx);
            self._next_import_func += 1;
        }

        // Add main export (must be known now).
        let main_idx =
            main_wasm_idx.context("backend-wasm expects lowered module entry function")?;
        if self.mode == CompileMode::Eval {
            self.exports
                .export("__eval_entry", ExportKind::Func, main_idx);
        } else {
            self.exports.export("main", ExportKind::Func, main_idx);
        }

        // Reserve indices for object helper functions (so they're known during user function compilation).
        if self.mode == CompileMode::Normal {
            let support_import_base = host_import_specs().len() as u32;
            self.obj_new_func_idx = support_import_base + 0;
            self.obj_get_func_idx = support_import_base + 1;
            self.obj_set_func_idx = support_import_base + 2;
            self.obj_delete_func_idx = support_import_base + 3;
            self.arr_new_func_idx = support_import_base + 4;
            self.elem_get_func_idx = support_import_base + 5;
            self.elem_set_func_idx = support_import_base + 6;
            self.string_eq_func_idx = support_import_base + 7;
            self.to_int32_func_idx = support_import_base + 8;
            self.get_proto_from_ctor_func_idx = support_import_base + 9;
            for i in 0..10u32 {
                self.push_func_table(support_import_base + i);
            }
        } else {
            self.obj_new_func_idx = self._next_import_func;
            self.functions.function(7);
            self.push_func_table(self._next_import_func);
            self._next_import_func += 1;
            self.obj_get_func_idx = self._next_import_func;
            self.functions.function(8);
            self.push_func_table(self._next_import_func);
            self._next_import_func += 1;
            self.obj_set_func_idx = self._next_import_func;
            self.functions.function(9);
            self.push_func_table(self._next_import_func);
            self._next_import_func += 1;
            self.obj_delete_func_idx = self._next_import_func;
            self.functions.function(8);
            self.push_func_table(self._next_import_func);
            self._next_import_func += 1;
            self.to_int32_func_idx = self._next_import_func;
            self.functions.function(10);
            self.push_func_table(self._next_import_func);
            self._next_import_func += 1;
            self.string_eq_func_idx = self._next_import_func;
            self.functions.function(26);
            self.push_func_table(self._next_import_func);
            self._next_import_func += 1;
            self.arr_new_func_idx = self._next_import_func;
            self.functions.function(7);
            self.push_func_table(self._next_import_func);
            self._next_import_func += 1;
            self.elem_get_func_idx = self._next_import_func;
            self.functions.function(8);
            self.push_func_table(self._next_import_func);
            self._next_import_func += 1;
            self.elem_set_func_idx = self._next_import_func;
            self.functions.function(9);
            self.push_func_table(self._next_import_func);
            self._next_import_func += 1;
            self.get_proto_from_ctor_func_idx = self._next_import_func;
            self.functions.function(3);
            self.push_func_table(self._next_import_func);
            self._next_import_func += 1;
        }
        let arr_proto_base = self.function_table.len() as u32;
        for (idx, _) in array_proto_method_specs() {
            self.push_func_table(idx as u32);
        }
        self.arr_proto_table_base = arr_proto_base;

        if self.mode == CompileMode::Normal {
            // P2.2: __wjsm_init_globals — 在 bootstrap 之前由 runtime 调用，
            // 设置所有 imported globals 的初始值（heap 布局等编译期计算值）。
            // 必须在 initialize_host_post_bootstrap 之前执行，因为 host 函数
            // 依赖 heap_ptr/obj_table_ptr 等全局的正确值。
            self.init_globals_func_idx = self._next_import_func;
            self.functions.function(4); // () -> i64
            self._next_import_func += 1;
            self.exports.export(
                "__wjsm_init_globals",
                ExportKind::Func,
                self.init_globals_func_idx,
            );

            // Startup snapshot 边界：把 primordial bootstrap 与当前模块函数属性初始化拆成可单独调用的阶段。
            self.bootstrap_func_idx = self._next_import_func;
            self.functions.function(4); // () -> i64
            self._next_import_func += 1;

            self.init_function_props_func_idx = self._next_import_func;
            self.functions.function(4); // () -> i64
            self._next_import_func += 1;
            self.exports.export(
                "__wjsm_bootstrap_once",
                ExportKind::Func,
                self.bootstrap_func_idx,
            );
            self.exports.export(
                "__wjsm_init_function_props",
                ExportKind::Func,
                self.init_function_props_func_idx,
            );
        }

        // Pre-write typeof type strings to data segment start (nul-terminated)
        // 必须在编译用户函数之前设置，否则 encode_constant 会从 offset 0 开始分配字符串，
        // 随后 typeof 字符串会覆盖用户字符串数据。
        let typeof_strings: &[(u32, &str)] = &[
            (constants::TYPEOF_UNDEFINED_OFFSET, "undefined"),
            (constants::TYPEOF_OBJECT_OFFSET, "object"),
            (constants::TYPEOF_BOOLEAN_OFFSET, "boolean"),
            (constants::TYPEOF_STRING_OFFSET, "string"),
            (constants::TYPEOF_FUNCTION_OFFSET, "function"),
            (constants::TYPEOF_NUMBER_OFFSET, "number"),
            (constants::TYPEOF_SYMBOL_OFFSET, "symbol"),
            (constants::TYPEOF_BIGINT_OFFSET, "bigint"),
        ];
        for &(offset, s) in typeof_strings {
            let end = offset as usize + s.len() + 1;
            if self.string_data.len() < end {
                self.string_data.resize(end, 0);
            }
            self.string_data[offset as usize..offset as usize + s.len()]
                .copy_from_slice(s.as_bytes());
            self.string_data[offset as usize + s.len()] = 0;
            self.string_ptr_cache
                .insert(s.to_string(), self.data_base + offset);
        }

        // Pre-write property descriptor strings after typeof strings
        // 用于 Object.getOwnPropertyDescriptor 返回的描述符对象
        let prop_desc_strings: &[(u32, &str)] = &[
            (constants::PROP_DESC_VALUE_OFFSET, "value"),
            (constants::PROP_DESC_WRITABLE_OFFSET, "writable"),
            (constants::PROP_DESC_ENUMERABLE_OFFSET, "enumerable"),
            (constants::PROP_DESC_CONFIGURABLE_OFFSET, "configurable"),
            (constants::PROP_DESC_GET_OFFSET, "get"),
            (constants::PROP_DESC_SET_OFFSET, "set"),
        ];
        for &(offset, s) in prop_desc_strings {
            let end = offset as usize + s.len() + 1;
            if self.string_data.len() < end {
                self.string_data.resize(end, 0);
            }
            self.string_data[offset as usize..offset as usize + s.len()]
                .copy_from_slice(s.as_bytes());
            self.string_data[offset as usize + s.len()] = 0;
            self.string_ptr_cache
                .insert(s.to_string(), self.data_base + offset);
        }

        let promise_strings: &[(u32, &str)] = &[
            (constants::PROMISE_STATE_PENDING_OFFSET, "pending"),
            (constants::PROMISE_STATE_FULFILLED_OFFSET, "fulfilled"),
            (constants::PROMISE_STATE_REJECTED_OFFSET, "rejected"),
            (constants::PROMISE_THEN_OFFSET, "then"),
            (constants::PROMISE_CATCH_OFFSET, "catch"),
            (constants::PROMISE_FINALLY_OFFSET, "finally"),
            (constants::PROMISE_RESOLVE_OFFSET, "resolve"),
            (constants::PROMISE_REJECT_OFFSET, "reject"),
            (constants::PROMISE_ALL_OFFSET, "all"),
            (constants::PROMISE_RACE_OFFSET, "race"),
            (constants::PROMISE_ALLSETTLED_OFFSET, "allSettled"),
            (constants::PROMISE_ANY_OFFSET, "any"),
            (constants::PROMISE_CONSTRUCTOR_OFFSET, "constructor"),
            (constants::ASYNC_ITERATOR_OFFSET, "asyncIterator"),
        ];
        for &(offset, s) in promise_strings {
            let end = offset as usize + s.len() + 1;
            if self.string_data.len() < end {
                self.string_data.resize(end, 0);
            }
            self.string_data[offset as usize..offset as usize + s.len()]
                .copy_from_slice(s.as_bytes());
            self.string_data[offset as usize + s.len()] = 0;
            self.string_ptr_cache
                .insert(s.to_string(), self.data_base + offset);
        }

        // Pre-write primordial property names used by bootstrap, function-props,
        // and host post-bootstrap (Array.prototype methods, length, name,
        // toStringTag, etc.). Fixed offsets ensure name_ids are consistent
        // across different user source compilations — required for snapshot ABI.
        for (offset, s) in constants::primordial_string_offsets() {
            let end = *offset as usize + s.len() + 1;
            if self.string_data.len() < end {
                self.string_data.resize(end, 0);
            }
            self.string_data[*offset as usize..*offset as usize + s.len()]
                .copy_from_slice(s.as_bytes());
            self.string_data[*offset as usize + s.len()] = 0;
            self.string_ptr_cache
                .insert(s.to_string(), self.data_base + *offset);
        }

        self.data_offset = constants::USER_STRING_START;
        // 填充 string_data 到 data_offset，确保后续用户字符串追加到正确偏移量
        self.string_data.resize(self.data_offset as usize, 0);

        // Assign global indices before compile_object_helpers needs them.
        self.func_props_global_idx = 0;
        self.heap_ptr_global_idx = 1;
        self.obj_table_global_idx = 2;
        self.obj_table_count_global_idx = 3;
        self.num_ir_functions = module.functions().len() as u32;
        self.shadow_sp_global_idx = 4;
        self.alloc_counter_global_idx = 5;
        self.array_proto_handle_global_idx = 9;
        self.object_proto_handle_global_idx = 10;
        self.eval_var_map_ptr_global_idx = 11;
        self.eval_var_map_count_global_idx = 12;
        self.bootstrap_done_global_idx = 13;
        self.function_props_done_global_idx = 14;
        self.function_props_base_global_idx = 15;
        self.arr_proto_table_base_global_idx = 16;
        self.arr_proto_table_len_global_idx = 17;
        self.arr_proto_table_hash_global_idx = 18;

        // Record user function base index (after all imports + helpers)
        self.user_func_base_idx = self._next_import_func;
        for (function_id, function) in module.functions().iter().enumerate() {
            if is_module_entry_ir_function(function.name()) {
                self.compile_function(module, function)?;
            } else {
                self.compile_js_function(
                    module,
                    function,
                    wjsm_ir::FunctionId(function_id as u32),
                )?;
            }
        }

        self.compile_number_proto_wrappers();

        // P2.2 后 heap 布局由 imported globals 显式初始化。计算 heap_start 之前
        // 必须先固化全部 data segment；否则后续追加的函数名字符串或 eval metadata
        // 会落进 object heap，被分配/GC 覆盖。
        self.finalize_eval_var_map_data();
        self.intern_data_string("length");
        self.intern_data_string("name");
        for function_name in self.function_names.clone() {
            self.intern_data_string(&function_name);
        }

        // P2.2: 提前计算 heap 布局，供 bootstrap 函数中的 emit_globals_init 使用。
        // 这些值原本在 globals 定义段中计算，现在 globals 是 import 的，
        // 需要在编译 bootstrap 之前确定初始值。
        let heap_start = (self.data_offset + 7) & !7; // align to 8 bytes
        let num_functions = self.num_ir_functions;
        let handle_table_entries = std::cmp::max(2048, num_functions * 2);
        let handle_table_size = handle_table_entries * 4;
        let shadow_stack_base = heap_start + handle_table_size;
        let object_heap_start = shadow_stack_base + SHADOW_STACK_SIZE;
        let shadow_stack_end = shadow_stack_base + SHADOW_STACK_SIZE;
        if self.mode == CompileMode::Normal {
            self.normal_init_values = Some(NormalGlobalsInit {
                heap_ptr: object_heap_start as i32,
                obj_table_ptr: heap_start as i32,
                shadow_sp: shadow_stack_base as i32,
                object_heap_start: object_heap_start as i32,
                num_ir_functions: num_functions as i32,
                shadow_stack_end: shadow_stack_end as i32,
                eval_var_map_ptr: self.eval_var_map_ptr as i32,
                eval_var_map_count: self.eval_var_map_count as i32,
                arr_proto_table_base: self.arr_proto_table_base as i32,
                arr_proto_table_len: array_proto_table_len() as i32,
                arr_proto_table_hash: array_proto_table_hash() as i64,
            });
        }

        // Pass 3: Compile helper functions.
        if self.mode == CompileMode::Eval {
            self.compile_object_helpers();
        }
        if self.mode == CompileMode::Eval {
            self.compile_array_helpers();
        }
        if self.mode == CompileMode::Eval {
            self.compile_get_proto_from_ctor();
        }
        if self.mode == CompileMode::Normal {
            self.compile_init_globals_function();
            self.compile_bootstrap_once_function();
            self.compile_init_function_props_function();
        }
        if self.mode == CompileMode::Eval {
            // Eval mode: 定义自己的 table
            self.table.table(TableType {
                element_type: RefType::FUNCREF,
                minimum: self.function_table.len() as u64,
                maximum: None,
                table64: false,
                shared: false,
            });
            self.elements.active(
                Some(0),
                &ConstExpr::i32_const(0),
                Elements::Functions(std::borrow::Cow::Borrowed(&self.function_table)),
            );
        } else {
            // Normal mode (P2.2): table 是 import 的（env.__table）。
            // element section 从 table[0] 开始填充。support module 不使用 element section，
            // 所以 table 完全由 user wasm 使用。
            self.elements.active(
                Some(0),
                &ConstExpr::i32_const(0),
                Elements::Functions(std::borrow::Cow::Borrowed(&self.function_table)),
            );
        }

        if self.mode == CompileMode::Eval {
            self.exports.export("__func_props", ExportKind::Global, 0);
            self.exports.export("__heap_ptr", ExportKind::Global, 1);
            self.exports
                .export("__obj_table_ptr", ExportKind::Global, 2);
            self.exports
                .export("__obj_table_count", ExportKind::Global, 3);
            self.exports.export("__shadow_sp", ExportKind::Global, 4);
            self.exports
                .export("__alloc_counter", ExportKind::Global, 5);
            self.exports
                .export("__object_heap_start", ExportKind::Global, 6);
            self.exports
                .export("__num_ir_functions", ExportKind::Global, 7);
            self.exports
                .export("__shadow_stack_end", ExportKind::Global, 8);
            self.exports
                .export("__array_proto_handle", ExportKind::Global, 9);
            self.exports
                .export("__object_proto_handle", ExportKind::Global, 10);
            self.exports.export(
                "__eval_var_map_ptr",
                ExportKind::Global,
                self.eval_var_map_ptr_global_idx,
            );
            self.exports.export(
                "__eval_var_map_count",
                ExportKind::Global,
                self.eval_var_map_count_global_idx,
            );
            self.exports.export(
                "__bootstrap_done",
                ExportKind::Global,
                self.bootstrap_done_global_idx,
            );
            self.exports.export(
                "__function_props_done",
                ExportKind::Global,
                self.function_props_done_global_idx,
            );
            self.exports.export(
                "__function_props_base",
                ExportKind::Global,
                self.function_props_base_global_idx,
            );
            self.exports.export(
                "__arr_proto_table_base",
                ExportKind::Global,
                self.arr_proto_table_base_global_idx,
            );
            self.exports.export(
                "__arr_proto_table_len",
                ExportKind::Global,
                self.arr_proto_table_len_global_idx,
            );
            self.exports.export(
                "__arr_proto_table_hash",
                ExportKind::Global,
                self.arr_proto_table_hash_global_idx,
            );
        }
        if !self.string_data.is_empty() {
            self.data.active(
                0,
                &ConstExpr::i32_const(self.data_base as i32),
                self.string_data.clone(),
            );
        }
        Ok(())
    }

    /// P2.2: 在 main prologue 中初始化所有 imported globals。
    /// 这些值原本通过 ConstExpr 在 global 定义时设置，改为 import 后必须显式 global.set。
    /// 只在 main 函数开始时调用一次，在任何 helper 调用之前。
    fn emit_globals_init(&mut self) {
        let init = match &self.normal_init_values {
            Some(v) => *v,
            None => return,
        };
        // global 0: __func_props = 0 (deprecated)
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::GlobalSet(0));
        // global 1: __heap_ptr
        self.emit(WasmInstruction::I32Const(init.heap_ptr));
        self.emit(WasmInstruction::GlobalSet(1));
        // global 2: __obj_table_ptr
        self.emit(WasmInstruction::I32Const(init.obj_table_ptr));
        self.emit(WasmInstruction::GlobalSet(2));
        // global 3: __obj_table_count = 0
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::GlobalSet(3));
        // global 4: __shadow_sp
        self.emit(WasmInstruction::I32Const(init.shadow_sp));
        self.emit(WasmInstruction::GlobalSet(4));
        // global 5: __alloc_counter = 0
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::GlobalSet(5));
        // global 6: __object_heap_start
        self.emit(WasmInstruction::I32Const(init.object_heap_start));
        self.emit(WasmInstruction::GlobalSet(6));
        // global 7: __num_ir_functions
        self.emit(WasmInstruction::I32Const(init.num_ir_functions));
        self.emit(WasmInstruction::GlobalSet(7));
        // global 8: __shadow_stack_end
        self.emit(WasmInstruction::I32Const(init.shadow_stack_end));
        self.emit(WasmInstruction::GlobalSet(8));
        // global 9: __array_proto_handle = -1 (uninitialized)
        self.emit(WasmInstruction::I32Const(-1));
        self.emit(WasmInstruction::GlobalSet(9));
        // global 10: __object_proto_handle = -1 (uninitialized)
        self.emit(WasmInstruction::I32Const(-1));
        self.emit(WasmInstruction::GlobalSet(10));
        // global 11: __eval_var_map_ptr
        self.emit(WasmInstruction::I32Const(init.eval_var_map_ptr));
        self.emit(WasmInstruction::GlobalSet(11));
        // global 12: __eval_var_map_count
        self.emit(WasmInstruction::I32Const(init.eval_var_map_count));
        self.emit(WasmInstruction::GlobalSet(12));
        // global 13: __bootstrap_done = 0
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::GlobalSet(13));
        // global 14: __function_props_done = 0
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::GlobalSet(14));
        // global 15: __function_props_base = 0
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::GlobalSet(15));
        // global 16: __arr_proto_table_base
        self.emit(WasmInstruction::I32Const(init.arr_proto_table_base));
        self.emit(WasmInstruction::GlobalSet(16));
        // global 17: __arr_proto_table_len
        self.emit(WasmInstruction::I32Const(init.arr_proto_table_len));
        self.emit(WasmInstruction::GlobalSet(17));
        // global 18: __arr_proto_table_hash
        self.emit(WasmInstruction::I64Const(init.arr_proto_table_hash));
        self.emit(WasmInstruction::GlobalSet(18));
    }

    fn compile_init_globals_function(&mut self) {
        let previous_shadow_sp_scratch_idx = self.shadow_sp_scratch_idx;
        self.shadow_sp_scratch_idx = 0;
        self.current_func = Some(Function::new(vec![(1, ValType::I32)]));

        // 设置所有 imported globals 的初始值
        self.emit_globals_init();

        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        self.emit(WasmInstruction::End);

        self.codes.function(
            self.current_func
                .as_ref()
                .expect("init_globals function should be initialized"),
        );
        self.current_func = None;
        self.shadow_sp_scratch_idx = previous_shadow_sp_scratch_idx;
    }

    fn emit_startup_phase_call(&mut self, func_idx: u32) {
        self.emit(WasmInstruction::Call(func_idx));
        self.emit(WasmInstruction::LocalTee(self.string_concat_scratch_idx));
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

    fn compile_bootstrap_once_function(&mut self) {
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
        // ── 初始化 Array.prototype ──
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

        // ── 初始化 Object.prototype ──
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

    fn compile_init_function_props_function(&mut self) {
        let previous_shadow_sp_scratch_idx = self.shadow_sp_scratch_idx;
        self.shadow_sp_scratch_idx = 0;
        self.current_func = Some(Function::new(vec![(1, ValType::I32)]));

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

            self.emit(WasmInstruction::GlobalGet(
                self.function_props_base_global_idx,
            ));
            self.emit(WasmInstruction::GlobalSet(self.obj_table_count_global_idx));
        }

        let length_name_id = self.intern_data_string("length");
        let name_name_id = self.intern_data_string("name");
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

    pub(crate) fn compile_function(
        &mut self,
        module: &IrModule,
        function: &IrFunction,
    ) -> Result<()> {
        self.current_func_is_main = is_module_entry_ir_function(function.name());
        self.current_func_returns_value =
            self.mode == CompileMode::Eval || self.current_func_is_main;
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
        self.string_concat_scratch_idx = local_count;
        self.shadow_sp_scratch_idx = local_count + 2;
        self.safepoint_sp_saved_idx = local_count + 4;
        self.eval_var_base_local_idx = local_count + 5;
        let param_i64_count = self.ssa_local_base;
        let total_i64_locals = local_count.saturating_sub(param_i64_count) + 2; // string_concat + call_env_obj
        let total_i32_locals = 3 + u32::from(!self.var_memory_offsets.is_empty());
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
        self.current_fn_liveness = None;
        self.current_fn_value_ty = None;
        self.current_fn_var_liveness = None;
        self.current_fn_var_ty = None;

        Ok(())
    }

    pub(crate) fn compile_js_function(
        &mut self,
        module: &IrModule,
        function: &IrFunction,
        function_id: wjsm_ir::FunctionId,
    ) -> Result<()> {
        self.current_func_returns_value = true;
        self.current_home_object = function.home_object;
        self.current_function_id = Some(function_id);
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

        // ── GC safepoint（P2）：计算 liveness + ValueTy（per-ValueId + 变量）──
        self.setup_gc_safepoint_analysis(module, function);
        self.current_emit_block_idx = 0;
        self.current_emit_instr_idx = 0;

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

        // scratch locals: 所有 i64 在前，然后所有 i32（WASM locals 按 type 分组）
        // string_concat (i64) at total_locals
        // call_env_obj (i64) at total_locals+1
        // shadow_sp (i32) at total_locals+2
        // call_func_idx (i32) at total_locals+3
        // safepoint_sp_saved (i32) at total_locals+4  (P2: safepoint spill save/restore)
        self.string_concat_scratch_idx = total_locals;
        // call_env_obj scratch = string_concat + 1 (i64), computed by call_env_obj_scratch()
        self.shadow_sp_scratch_idx = total_locals + 2;
        self.safepoint_sp_saved_idx = total_locals + 4;
        self.eval_var_base_local_idx = total_locals + 5;
        // call_func_idx = shadow_sp + 1 (i32), computed by call_func_idx_scratch()
        let total_i64_locals = total_locals.saturating_sub(4) + 2; // string_concat + call_env_obj
        let total_i32_locals = 3 + u32::from(!self.var_memory_offsets.is_empty());

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

        self.emit_init_module_global_for_js_function(function);
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
        self.current_home_object = None;
        self.current_function_id = None;
        self.current_fn_liveness = None;
        self.current_fn_value_ty = None;
        self.current_fn_var_liveness = None;
        self.current_fn_var_ty = None;

        Ok(())
    }
}
