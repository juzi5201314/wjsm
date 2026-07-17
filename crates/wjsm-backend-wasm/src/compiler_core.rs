use super::*;
use crate::host_import_registry::{HostImportKey, host_import_specs};

/// 27 个 env global 的 export 名称，与 support module abi::ENV_GLOBALS 对齐。
/// 用于 Normal mode user wasm re-export imported globals。
const ENV_GLOBAL_EXPORT_NAMES: &[&str] = &[
    "__func_props",
    "__heap_ptr",
    "__obj_table_ptr",
    "__obj_table_count",
    "__shadow_sp",
    "__object_heap_start",
    "__num_ir_functions",
    "__shadow_stack_end",
    "__array_proto_handle",
    "__object_proto_handle",
    "__eval_var_map_ptr",
    "__eval_var_map_count",
    "__bootstrap_done",
    "__function_props_done",
    "__function_props_base",
    "__arr_proto_table_base",
    "__arr_proto_table_len",
    "__arr_proto_table_hash",
    "__heap_limit",
    "__alloc_ptr",
    "__alloc_end",
    "__gc_alloc_bytes",
    "__gc_trigger_bytes",
    "__gc_phase",
    "__good_color",
    "__barrier_buf_ptr",
    "__barrier_buf_end",
];

impl Compiler {
    pub(crate) fn push_func_table(&mut self, wasm_idx: u32) {
        let table_pos = self.table_base + self.function_table.len() as u32;
        self.function_table_reverse.insert(wasm_idx, table_pos);
        self.function_table.push(wasm_idx);
    }

    pub(crate) fn new_with_layout(mode: CompileMode, data_base: u32, table_base: u32) -> Self {
        let types = crate::shared_types::build_shared_type_section();
        let mut imports = ImportSection::new();
        for spec in host_import_specs() {
            imports.import("env", spec.name, EntityType::Function(spec.type_idx));
        }
        if mode == CompileMode::Eval {
            imports.import(
                "env",
                "memory",
                EntityType::Memory(MemoryType {
                    minimum: 8,
                    maximum: None,
                    memory64: false,
                    shared: false,
                    page_size_log2: None,
                }),
            );
            // 独立影子栈 memory（index 1）。
            imports.import(
                "env",
                crate::SHADOW_MEMORY_NAME,
                EntityType::Memory(MemoryType {
                    minimum: 1,
                    maximum: None,
                    memory64: false,
                    shared: false,
                    page_size_log2: None,
                }),
            );
            #[cfg(feature = "managed-heap-v2")]
            imports.import(
                "env",
                wjsm_ir::HEAP_MEMORY_NAME,
                EntityType::Memory(MemoryType {
                    minimum: wjsm_ir::HEAP_MEMORY_MIN_PAGES,
                    maximum: Some(wjsm_ir::HEAP_MEMORY_MAX_PAGES),
                    memory64: true,
                    shared: true,
                    page_size_log2: None,
                }),
            );
            // P2.2 后父模块把 memory/table/globals 全部作为 mutable env import
            // 再 re-export；compiled eval 手动实例化时导入同一批 global，mutability
            // 必须与父模块 export 完全一致，否则 wasmtime 会拒绝实例化并退回解释器路径。
            import_eval_global(&mut imports, "__func_props", ValType::I32, true);
            import_eval_global(&mut imports, "__heap_ptr", ValType::I32, true);
            import_eval_global(&mut imports, "__obj_table_ptr", ValType::I32, true);
            import_eval_global(&mut imports, "__obj_table_count", ValType::I32, true);
            import_eval_global(&mut imports, "__shadow_sp", ValType::I32, true);
            import_eval_global(&mut imports, "__object_heap_start", ValType::I32, true);
            import_eval_global(&mut imports, "__num_ir_functions", ValType::I32, true);
            import_eval_global(&mut imports, "__shadow_stack_end", ValType::I32, true);
            import_eval_global(&mut imports, "__array_proto_handle", ValType::I32, true);
            import_eval_global(&mut imports, "__object_proto_handle", ValType::I32, true);
            import_eval_global(&mut imports, "__eval_var_map_ptr", ValType::I32, true);
            import_eval_global(&mut imports, "__eval_var_map_count", ValType::I32, true);
            import_eval_global(&mut imports, "__bootstrap_done", ValType::I32, true);
            import_eval_global(&mut imports, "__function_props_done", ValType::I32, true);
            import_eval_global(&mut imports, "__function_props_base", ValType::I32, true);
            import_eval_global(&mut imports, "__arr_proto_table_base", ValType::I32, true);
            import_eval_global(&mut imports, "__arr_proto_table_len", ValType::I32, true);
            import_eval_global(&mut imports, "__arr_proto_table_hash", ValType::I64, true);
            import_eval_global(&mut imports, "__heap_limit", ValType::I32, true);
            import_eval_global(&mut imports, "__alloc_ptr", ValType::I32, true);
            import_eval_global(&mut imports, "__alloc_end", ValType::I32, true);
            import_eval_global(&mut imports, "__gc_alloc_bytes", ValType::I32, true);
            import_eval_global(&mut imports, "__gc_trigger_bytes", ValType::I32, true);
            import_eval_global(&mut imports, "__gc_phase", ValType::I32, true);
            import_eval_global(&mut imports, "__good_color", ValType::I32, true);
            import_eval_global(&mut imports, "__barrier_buf_ptr", ValType::I32, true);
            import_eval_global(&mut imports, "__barrier_buf_end", ValType::I32, true);
            #[cfg(feature = "managed-heap-v2")]
            import_v2_heap_globals(&mut imports);
            #[cfg(feature = "managed-heap-v2")]
            import_support_helpers(&mut imports);
            // 与 runtime module 一样导入父模块 __table，使 FunctionRef 使用主表下标。
            imports.import(
                "env",
                "__table",
                EntityType::Table(TableType {
                    element_type: RefType::FUNCREF,
                    minimum: 0,
                    maximum: None,
                    table64: false,
                    shared: false,
                }),
            );
        } else {
            // Normal mode: import memory + table + 27 globals from env，
            // 与 support module 共享同一份运行时状态。runtime 在 instantiate 前创建
            // memory/table/globals 并通过 Linker 注册到 env namespace。
            imports.import(
                "env",
                "memory",
                EntityType::Memory(MemoryType {
                    minimum: 8,
                    maximum: None,
                    memory64: false,
                    shared: false,
                    page_size_log2: None,
                }),
            );
            // 独立影子栈 memory（index 1）。
            imports.import(
                "env",
                crate::SHADOW_MEMORY_NAME,
                EntityType::Memory(MemoryType {
                    minimum: 1,
                    maximum: None,
                    memory64: false,
                    shared: false,
                    page_size_log2: None,
                }),
            );
            #[cfg(feature = "managed-heap-v2")]
            imports.import(
                "env",
                wjsm_ir::HEAP_MEMORY_NAME,
                EntityType::Memory(MemoryType {
                    minimum: wjsm_ir::HEAP_MEMORY_MIN_PAGES,
                    maximum: Some(wjsm_ir::HEAP_MEMORY_MAX_PAGES),
                    memory64: true,
                    shared: true,
                    page_size_log2: None,
                }),
            );
            imports.import(
                "env",
                "__table",
                EntityType::Table(TableType {
                    element_type: RefType::FUNCREF,
                    minimum: 0,
                    maximum: None,
                    table64: false,
                    shared: false,
                }),
            );
            // 27 个 env globals — 与 support module 的 abi::ENV_GLOBALS 完全对齐。
            // 全部 mutable：user wasm 在 bootstrap 中用 global.set 初始化。
            import_eval_global(&mut imports, "__func_props", ValType::I32, true);
            import_eval_global(&mut imports, "__heap_ptr", ValType::I32, true);
            import_eval_global(&mut imports, "__obj_table_ptr", ValType::I32, true);
            import_eval_global(&mut imports, "__obj_table_count", ValType::I32, true);
            import_eval_global(&mut imports, "__shadow_sp", ValType::I32, true);
            import_eval_global(&mut imports, "__object_heap_start", ValType::I32, true);
            import_eval_global(&mut imports, "__num_ir_functions", ValType::I32, true);
            import_eval_global(&mut imports, "__shadow_stack_end", ValType::I32, true);
            import_eval_global(&mut imports, "__array_proto_handle", ValType::I32, true);
            import_eval_global(&mut imports, "__object_proto_handle", ValType::I32, true);
            import_eval_global(&mut imports, "__eval_var_map_ptr", ValType::I32, true);
            import_eval_global(&mut imports, "__eval_var_map_count", ValType::I32, true);
            import_eval_global(&mut imports, "__bootstrap_done", ValType::I32, true);
            import_eval_global(&mut imports, "__function_props_done", ValType::I32, true);
            import_eval_global(&mut imports, "__function_props_base", ValType::I32, true);
            import_eval_global(&mut imports, "__arr_proto_table_base", ValType::I32, true);
            import_eval_global(&mut imports, "__arr_proto_table_len", ValType::I32, true);
            import_eval_global(&mut imports, "__arr_proto_table_hash", ValType::I64, true);
            import_eval_global(&mut imports, "__heap_limit", ValType::I32, true);
            import_eval_global(&mut imports, "__alloc_ptr", ValType::I32, true);
            import_eval_global(&mut imports, "__alloc_end", ValType::I32, true);
            import_eval_global(&mut imports, "__gc_alloc_bytes", ValType::I32, true);
            import_eval_global(&mut imports, "__gc_trigger_bytes", ValType::I32, true);
            import_eval_global(&mut imports, "__gc_phase", ValType::I32, true);
            import_eval_global(&mut imports, "__good_color", ValType::I32, true);
            import_eval_global(&mut imports, "__barrier_buf_ptr", ValType::I32, true);
            import_eval_global(&mut imports, "__barrier_buf_end", ValType::I32, true);
            #[cfg(feature = "managed-heap-v2")]
            import_v2_heap_globals(&mut imports);

            // Normal mode 与 Eval V2 共用同一 support helper ABI。
            import_support_helpers(&mut imports);
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
        if mode == CompileMode::Eval {
            // compiled eval 也会直接调用 host imports；host 侧统一用
            // WasmEnv::from_caller 取 memory/table/global，因此 eval module 必须
            // 重新 export 父模块传入的 memory 和本模块 table。
            exports.export("memory", ExportKind::Memory, 0);
            exports.export(crate::SHADOW_MEMORY_NAME, ExportKind::Memory, 1);
            #[cfg(feature = "managed-heap-v2")]
            exports.export(
                wjsm_ir::HEAP_MEMORY_NAME,
                ExportKind::Memory,
                wjsm_ir::HEAP_MEMORY_INDEX,
            );
            exports.export("__table", ExportKind::Table, 0);
            for (index, spec) in host_import_specs().iter().enumerate() {
                exports.export(spec.name, ExportKind::Func, index as u32);
            }
        } else {
            // Normal mode (P2.2)：re-export imported memory (idx 0) + shadow memory (idx 1)
            // + table (idx 0) + 27 globals (idx 0..26)。
            exports.export("memory", ExportKind::Memory, 0);
            exports.export(crate::SHADOW_MEMORY_NAME, ExportKind::Memory, 1);
            #[cfg(feature = "managed-heap-v2")]
            exports.export(
                wjsm_ir::HEAP_MEMORY_NAME,
                ExportKind::Memory,
                wjsm_ir::HEAP_MEMORY_INDEX,
            );
            exports.export("__table", ExportKind::Table, 0);
            for (g, name) in ENV_GLOBAL_EXPORT_NAMES.iter().enumerate() {
                exports.export(name, ExportKind::Global, g as u32);
            }
            #[cfg(feature = "managed-heap-v2")]
            for (offset, name) in [
                wjsm_ir::HEAP_ALLOC_PTR_GLOBAL_NAME,
                wjsm_ir::HEAP_ALLOC_END_GLOBAL_NAME,
                wjsm_ir::HEAP_OBJECT_START_GLOBAL_NAME,
                wjsm_ir::HEAP_LIMIT_GLOBAL_NAME,
            ]
            .iter()
            .enumerate()
            {
                exports.export(name, ExportKind::Global, 27 + offset as u32);
            }
            for (index, spec) in host_import_specs().iter().enumerate() {
                exports.export(spec.name, ExportKind::Func, index as u32);
            }
        }

        // Normal mode 不再定义自己的 memory（已 import）；Eval mode 也不定义。
        let memory = MemorySection::new();

        // Normal mode 与 V2 Eval 均导入 10 个 support helper。
        let helper_import_count =
            if mode == CompileMode::Normal || cfg!(feature = "managed-heap-v2") {
                10
            } else {
                0
            };
        let actual_import_count = host_import_specs().len() as u32 + helper_import_count;
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
            branch_inline_compiled: std::collections::HashSet::new(),
            loop_stack: Vec::new(),
            if_depth: 0,
            _next_import_func: actual_import_count,
            builtin_func_indices,
            special_host_import_indices,
            function_table: Vec::new(),
            table_base,
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
            computed_idx_scratch_idx: 0,
            eval_var_base_local_idx: 0,
            object_heap_start_global_idx: 5,
            num_ir_functions_global_idx: 6,
            shadow_stack_end_global_idx: 7,
            array_proto_handle_global_idx: 8,
            arr_proto_table_base: 0,
            arr_proto_table_base_global_idx: 15,
            arr_proto_table_len_global_idx: 16,
            arr_proto_table_hash_global_idx: 17,
            heap_limit_global_idx: 18,
            alloc_ptr_global_idx: 19,
            alloc_end_global_idx: 20,
            gc_alloc_bytes_global_idx: 21,
            gc_trigger_bytes_global_idx: 22,
            gc_phase_global_idx: 23,
            good_color_global_idx: 24,
            barrier_buf_ptr_global_idx: 25,
            barrier_buf_end_global_idx: 26,
            get_proto_from_ctor_func_idx: 0,
            string_eq_func_idx: 0,
            function_id_to_wasm_idx: HashMap::new(),
            object_proto_handle_global_idx: 9,
            bootstrap_done_global_idx: 12,
            function_props_done_global_idx: 13,
            function_props_base_global_idx: 14,
            init_globals_func_idx: 0,
            bootstrap_func_idx: 0,
            init_function_props_func_idx: 0,
            eval_var_map_ptr_global_idx: 10,
            eval_var_map_count_global_idx: 11,
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
            function_needs_prototype: Vec::new(),
            current_fn_liveness: None,
            current_fn_value_ty: None,
            current_fn_var_liveness: None,
            current_fn_var_ty: None,
            current_emit_block_idx: 0,
            current_emit_instr_idx: 0,
            gc_analysis: None,
            normal_init_values: None,
            source_file: None,
            source_map_entries: Vec::new(),
            debug: false,
            current_wasm_func_idx: 0,
            debug_emit_counter: 0,
            debug_line_entries: Vec::new(),
            debug_local_entries: Vec::new(),
            debug_debugger_pcs: Vec::new(),
        }
    }
}
