use super::*;
use crate::host_import_registry::{HostImportKey, host_import_specs};

/// 20 个 env global 的 export 名称，与 support module abi::ENV_GLOBALS 对齐。
/// 用于 Normal mode user wasm re-export imported globals。
const ENV_GLOBAL_EXPORT_NAMES: &[&str] = &[
    "__func_props",
    "__heap_ptr",
    "__obj_table_ptr",
    "__obj_table_count",
    "__shadow_sp",
    "__alloc_counter",
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
];

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
                    minimum: 4,
                    maximum: None,
                    memory64: false,
                    shared: false,
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
            import_eval_global(&mut imports, "__alloc_counter", ValType::I32, true);
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
        } else {
            // Normal mode (P2.2): import memory + table + 20 globals from env，
            // 与 support module 共享同一份运行时状态。runtime 在 instantiate 前创建
            // memory/table/globals 并通过 Linker 注册到 env namespace。
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
            // 20 个 env globals — 与 support module 的 abi::ENV_GLOBALS 完全对齐。
            // 全部 mutable：P2.2 后 user wasm 在 bootstrap 中用 global.set 初始化。
            import_eval_global(&mut imports, "__func_props", ValType::I32, true);
            import_eval_global(&mut imports, "__heap_ptr", ValType::I32, true);
            import_eval_global(&mut imports, "__obj_table_ptr", ValType::I32, true);
            import_eval_global(&mut imports, "__obj_table_count", ValType::I32, true);
            import_eval_global(&mut imports, "__shadow_sp", ValType::I32, true);
            import_eval_global(&mut imports, "__alloc_counter", ValType::I32, true);
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

            // P2.5+: import obj_*/arr_*/elem_*/string_eq/to_int32/get_proto_from_ctor from wjsm_support。
            // 10 个 helper imports 替代原来的 inline 函数定义。
            imports.import("wjsm_support", "obj_new", EntityType::Function(7));
            imports.import("wjsm_support", "obj_get", EntityType::Function(8));
            imports.import("wjsm_support", "obj_set", EntityType::Function(9));
            imports.import("wjsm_support", "obj_delete", EntityType::Function(8));
            imports.import("wjsm_support", "arr_new", EntityType::Function(7));
            imports.import("wjsm_support", "elem_get", EntityType::Function(8));
            imports.import("wjsm_support", "elem_set", EntityType::Function(9));
            imports.import("wjsm_support", "string_eq", EntityType::Function(26));
            imports.import("wjsm_support", "to_int32", EntityType::Function(10));
            imports.import(
                "wjsm_support",
                "get_proto_from_ctor",
                EntityType::Function(3),
            );
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
            exports.export("__table", ExportKind::Table, 0);
            for (index, spec) in host_import_specs().iter().enumerate() {
                exports.export(spec.name, ExportKind::Func, index as u32);
            }
        } else {
            // Normal mode (P2.2)：re-export imported memory (idx 0) + table (idx 0) +
            // 20 globals (idx 0..19)，使 WasmEnv::from_caller 仍能从 user instance
            // 的 exports 获取它们（零改动 host 函数）。
            exports.export("memory", ExportKind::Memory, 0);
            exports.export("__table", ExportKind::Table, 0);
            for (g, name) in ENV_GLOBAL_EXPORT_NAMES.iter().enumerate() {
                exports.export(name, ExportKind::Global, g as u32);
            }
            for (index, spec) in host_import_specs().iter().enumerate() {
                exports.export(spec.name, ExportKind::Func, index as u32);
            }
        }

        // Normal mode 不再定义自己的 memory（已 import）；Eval mode 也不定义。
        let memory = MemorySection::new();

        // Function import count: host funcs (+ Normal mode 的 10 support helper imports)
        let actual_import_count = if mode == CompileMode::Normal {
            host_import_specs().len() as u32 + 10
        } else {
            host_import_specs().len() as u32
        };
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
            alloc_counter_global_idx: 0,
            object_heap_start_global_idx: 6,
            num_ir_functions_global_idx: 7,
            shadow_stack_end_global_idx: 8,
            array_proto_handle_global_idx: 0,
            arr_proto_table_base: 0,
            arr_proto_table_base_global_idx: 16,
            arr_proto_table_len_global_idx: 17,
            arr_proto_table_hash_global_idx: 18,
            heap_limit_global_idx: 19,
            get_proto_from_ctor_func_idx: 0,
            string_eq_func_idx: 0,
            function_id_to_wasm_idx: HashMap::new(),
            object_proto_handle_global_idx: 0,
            bootstrap_done_global_idx: 13,
            function_props_done_global_idx: 14,
            function_props_base_global_idx: 15,
            init_globals_func_idx: 0,
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
            normal_init_values: None,
            allocation_sites: Vec::new(),
            next_allocation_site_id: FIRST_ALLOCATION_SITE_ID,
        }
    }
}
