use super::*;
use crate::host_import_registry::{HostImportKey, host_import_specs};

impl Compiler {
    pub(crate) fn push_func_table(&mut self, wasm_idx: u32) {
        let table_pos = self.function_table.len() as u32;
        self.function_table_reverse.insert(wasm_idx, table_pos);
        self.function_table.push(wasm_idx);
    }

    pub(crate) fn new(mode: CompileMode) -> Self {
        Self::new_with_data_base(mode, 0)
    }

    pub(crate) fn new_with_data_base(mode: CompileMode, data_base: u32) -> Self {
        let mut types = TypeSection::new();
        // Type 0: (i64) -> ()  — console_log
        types.ty().function(vec![ValType::I64], vec![]);
        // Type 1: () -> ()  — main
        types.ty().function(vec![], vec![]);
        // Type 2: (i64, i64) -> (i64)  — f64_mod, f64_pow
        types
            .ty()
            .function(vec![ValType::I64, ValType::I64], vec![ValType::I64]);
        // Type 3: (i64) -> (i64)  — iterator/enumerator helpers
        types.ty().function(vec![ValType::I64], vec![ValType::I64]);
        // Type 4: () -> (i64)  — (unused placeholder)
        types.ty().function(vec![], vec![ValType::I64]);
        // Type 5: (i64, i64) -> () — unused (was begin_try, now removed)
        types
            .ty()
            .function(vec![ValType::I64, ValType::I64], vec![]);
        // Type 6: (i64, i32, i32) -> (i64)  — JS function signature (shadow stack)
        //   param 0 = this_val (i64), param 1 = args_base_ptr (i32), param 2 = args_count (i32)
        types.ty().function(
            vec![ValType::I64, ValType::I32, ValType::I32],
            vec![ValType::I64],
        );
        // Type 7: (i32) -> (i32)  — $obj_new, $alloc
        types.ty().function(vec![ValType::I32], vec![ValType::I32]);
        // Type 8: (i64, i32) -> (i64)  — $obj_get (boxed object + key → value)
        types
            .ty()
            .function(vec![ValType::I64, ValType::I32], vec![ValType::I64]);
        // Type 9: (i64, i32, i64) -> ()  — $obj_set (boxed object + key + value)
        types
            .ty()
            .function(vec![ValType::I64, ValType::I32, ValType::I64], vec![]);
        // Type 10: (i64) -> (i32)  — $to_int32
        types.ty().function(vec![ValType::I64], vec![ValType::I32]);
        // Type 11: (i64, i64) -> (i64)  — string_concat
        types
            .ty()
            .function(vec![ValType::I64, ValType::I64], vec![ValType::I64]);
        // Type 12: (i64, i64, i32, i32) -> (i64) — JS 函数签名（含 env_obj）
        //   param 0 = env_obj (i64), param 1 = this_val (i64), param 2 = args_base_ptr (i32), param 3 = args_count (i32)
        types.ty().function(
            vec![ValType::I64, ValType::I64, ValType::I32, ValType::I32],
            vec![ValType::I64],
        );
        // Type 13: (i32, i64) -> (i64) — closure_create(func_idx, env_obj)
        types
            .ty()
            .function(vec![ValType::I32, ValType::I64], vec![ValType::I64]);
        // Type 14: (i32) -> (i32) — closure_get_func(closure_idx)
        types.ty().function(vec![ValType::I32], vec![ValType::I32]);
        // Type 15: (i32) -> (i64) — closure_get_env(closure_idx)
        types.ty().function(vec![ValType::I32], vec![ValType::I64]);
        // Type 16: (i64, i64, i64) -> (i64) — 3-arg array functions (indexOf, slice)
        types.ty().function(
            vec![ValType::I64, ValType::I64, ValType::I64],
            vec![ValType::I64],
        );
        // Type 17: (i64, i64, i64, i64) -> (i64) — 4-arg array functions (fill)
        types.ty().function(
            vec![ValType::I64, ValType::I64, ValType::I64, ValType::I64],
            vec![ValType::I64],
        );
        // Type 18: (i32, i32, i32) -> () — abort_shadow_stack_overflow
        types
            .ty()
            .function(vec![ValType::I32, ValType::I32, ValType::I32], vec![]);
        // Type 19: (i32, i32) -> (i64) — string_concat_va
        types
            .ty()
            .function(vec![ValType::I32, ValType::I32], vec![ValType::I64]);
        // Type 20: (i32, i32, i32, i32) -> (i64) — regex_create(pat_ptr, pat_len, flags_ptr, flags_len)
        types.ty().function(
            vec![ValType::I32, ValType::I32, ValType::I32, ValType::I32],
            vec![ValType::I64],
        );
        // Type 21: (i64) -> (i64) — async_function_start
        types.ty().function(vec![ValType::I64], vec![ValType::I64]);
        // Type 22: (i64, i64, i64, i64, i64) -> () — async_function_resume
        types.ty().function(
            vec![
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
            ],
            vec![],
        );
        // Type 23: (i64, i64, i64) -> () — async_function_suspend
        types
            .ty()
            .function(vec![ValType::I64, ValType::I64, ValType::I64], vec![]);
        // Type 24: (i64, i64, i64) -> (i64) — continuation_create
        types.ty().function(
            vec![ValType::I64, ValType::I64, ValType::I64],
            vec![ValType::I64],
        );
        // Type 25: (i64, i64, i64) -> () — continuation_save_var
        types
            .ty()
            .function(vec![ValType::I64, ValType::I64, ValType::I64], vec![]);
        // Type 26: (i32, i32) -> (i32) — nul-terminated string equality helper
        types
            .ty()
            .function(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
        // Type 27: (i64, i64, i64) -> (i64)  — jsx_create_element (tag, props, children)
        types.ty().function(
            vec![ValType::I64, ValType::I64, ValType::I64],
            vec![ValType::I64],
        );
        // Type 28: (i64, i64) -> (i64)  — proxy_create, proxy_revocable, reflect_has, etc.
        types
            .ty()
            .function(vec![ValType::I64, ValType::I64], vec![ValType::I64]);
        // Type 29: (i64, i64, i64) -> (i64)  — reflect_get, reflect_apply, etc.
        types.ty().function(
            vec![ValType::I64, ValType::I64, ValType::I64],
            vec![ValType::I64],
        );
        // Type 30: (i64, i64, i64, i64) -> (i64)  — reflect_set, reflect_define_property, etc.
        types.ty().function(
            vec![ValType::I64, ValType::I64, ValType::I64, ValType::I64],
            vec![ValType::I64],
        );
        // Type 31: (i64) -> (i64)  — reflect_is_extensible, reflect_own_keys, etc.
        types.ty().function(vec![ValType::I64], vec![ValType::I64]);
        // Type 32: (i64, i32, i64) -> (i64) — private_set(obj, key_name_id, value)
        types.ty().function(
            vec![ValType::I64, ValType::I32, ValType::I64],
            vec![ValType::I64],
        );
        // Type 33: (i32, i32) -> () — console varargs（args_base, args_count）
        types
            .ty()
            .function(vec![ValType::I32, ValType::I32], vec![]);
        // Type 34: (i64, i64, i64, i64, i64) -> (i64) — scope_record_add_binding (5 i64 args)
        types.ty().function(
            vec![
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
            ],
            vec![ValType::I64],
        );
        // Type 35: (i32, i32, i32) -> (i32) — gc_alloc_slow(size, heap_type, capacity) -> handle
        //   None（真 OOM）时 host 内部 trap（unreachable），故返回值总有效。
        types.ty().function(
            vec![ValType::I32, ValType::I32, ValType::I32],
            vec![ValType::I32],
        );
        // Type 36: () -> (i32) — gc_take_freed_handle() -> handle（-1 表空）
        types.ty().function(vec![], vec![ValType::I32]);
        let mut imports = ImportSection::new();
        for spec in host_import_specs() {
            imports.import("env", spec.name, EntityType::Function(spec.type_idx));
        }
        if mode == CompileMode::Eval {
            imports.import(
                "env",
                "memory",
                EntityType::Memory(MemoryType {
                    minimum: 4,
                    maximum: None,
                    memory64: false,
                    shared: false,
                    page_size_log2: None,
                }),
            );
            import_eval_global(&mut imports, "__func_props", ValType::I32, false);
            import_eval_global(&mut imports, "__heap_ptr", ValType::I32, true);
            import_eval_global(&mut imports, "__obj_table_ptr", ValType::I32, false);
            import_eval_global(&mut imports, "__obj_table_count", ValType::I32, true);
            import_eval_global(&mut imports, "__shadow_sp", ValType::I32, true);
            import_eval_global(&mut imports, "__alloc_counter", ValType::I32, true);
            import_eval_global(&mut imports, "__object_heap_start", ValType::I32, false);
            import_eval_global(&mut imports, "__num_ir_functions", ValType::I32, false);
            import_eval_global(&mut imports, "__shadow_stack_end", ValType::I32, false);
            import_eval_global(&mut imports, "__array_proto_handle", ValType::I32, true);
            import_eval_global(&mut imports, "__object_proto_handle", ValType::I32, true);
            import_eval_global(&mut imports, "__eval_var_map_ptr", ValType::I32, false);
            import_eval_global(&mut imports, "__eval_var_map_count", ValType::I32, false);
            // Startup snapshot 阶段全局（索引 13/14/15，与 Normal 模式一致）。eval 共享父模块
            // 的 obj_table，函数属性对象按父模块的 __function_props_base 重定位，故必须导入它；
            // bootstrap_done / function_props_done 仅为保持全局索引对齐而一并导入。
            import_eval_global(&mut imports, "__bootstrap_done", ValType::I32, true);
            import_eval_global(&mut imports, "__function_props_done", ValType::I32, true);
            import_eval_global(&mut imports, "__function_props_base", ValType::I32, true);
        }
        let mut builtin_func_indices = HashMap::new();
        let mut special_host_import_indices = HashMap::new();
        for (i, spec) in host_import_specs().iter().enumerate() {
            match spec.key {
                Some(HostImportKey::Builtin(b)) => {
                    builtin_func_indices.insert(b, i as u32);
                }
                Some(HostImportKey::Special(s)) => {
                    special_host_import_indices.insert(s, i as u32);
                }
                None => {}
            }
        }
        let functions = FunctionSection::new();

        let mut exports = ExportSection::new();
        exports.export("memory", ExportKind::Memory, 0);
        for (index, spec) in host_import_specs().iter().enumerate() {
            exports.export(spec.name, ExportKind::Func, index as u32);
        }

        let mut memory = MemorySection::new();
        if mode == CompileMode::Normal {
            memory.memory(MemoryType {
                minimum: 4, // 4 pages (256KB)：P4 GC obj_table 8KB + shadow stack 64KB + 对象堆
                maximum: None,
                memory64: false,
                shared: false,
                page_size_log2: None,
            });
        }

        // Count function imports only (memories/globals share separate index spaces)
        let actual_import_count = host_import_specs().len() as u32;
        Self {
            module: Module::new(),
            types,
            imports,
            functions,
            exports,
            codes: CodeSection::new(),
            memory,
            data: DataSection::new(),
            table: TableSection::new(),
            elements: ElementSection::new(),
            globals: GlobalSection::new(),
            current_func: None,
            string_data: Vec::new(),
            data_base,
            data_offset: 0,
            var_locals: HashMap::new(),
            var_memory_offsets: HashMap::new(),
            next_var_local: 0,
            phi_locals: HashMap::new(),
            compiled_blocks: std::collections::HashSet::new(),
            loop_stack: Vec::new(),
            if_depth: 0,
            _next_import_func: actual_import_count,
            builtin_func_indices,
            special_host_import_indices,
            function_table: Vec::new(),
            function_table_reverse: HashMap::new(),
            function_name_to_wasm_idx: HashMap::new(),
            obj_new_func_idx: 0,
            obj_get_func_idx: 0,
            obj_set_func_idx: 0,
            obj_delete_func_idx: 0,
            arr_new_func_idx: 0,
            elem_get_func_idx: 0,
            elem_set_func_idx: 0,
            to_int32_func_idx: 0,
            current_func_returns_value: false,
            current_func_is_main: false,
            user_func_base_idx: 0,
            heap_ptr_global_idx: 0,
            func_props_global_idx: 0,
            obj_table_global_idx: 0,
            obj_table_count_global_idx: 0,
            num_ir_functions: 0,
            ssa_local_base: 0,
            string_ptr_cache: HashMap::new(),
            string_concat_scratch_idx: 0,
            shadow_sp_global_idx: 0,
            shadow_sp_scratch_idx: 0,
            safepoint_sp_saved_idx: 0,
            eval_var_base_local_idx: 0,
            alloc_counter_global_idx: 0,
            object_heap_start_global_idx: 6,
            num_ir_functions_global_idx: 7,
            shadow_stack_end_global_idx: 8,
            array_proto_handle_global_idx: 0,
            arr_proto_table_base: 0,
            get_proto_from_ctor_func_idx: 0,
            string_eq_func_idx: 0,
            function_id_to_wasm_idx: HashMap::new(),
            object_proto_handle_global_idx: 0,
            bootstrap_done_global_idx: 13,
            function_props_done_global_idx: 14,
            function_props_base_global_idx: 15,
            bootstrap_func_idx: 0,
            init_function_props_func_idx: 0,
            eval_var_map_ptr_global_idx: 11,
            eval_var_map_count_global_idx: 12,
            eval_var_map_records: Vec::new(),
            eval_var_map_ptr: 0,
            eval_var_map_count: 0,
            continuation_local_idx: 0,
            current_function_has_eval: false,
            current_home_object: None,
            current_function_id: None,
            mode,
            function_param_counts: Vec::new(),
            function_names: Vec::new(),
            current_fn_liveness: None,
            current_fn_value_ty: None,
            current_fn_var_liveness: None,
            current_fn_var_ty: None,
            current_emit_block_idx: 0,
            current_emit_instr_idx: 0,
            gc_analysis: None,
        }
    }
}
