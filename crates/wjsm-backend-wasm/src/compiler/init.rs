use std::collections::HashMap;
use wasm_encoder::{
    CodeSection, DataSection, ElementSection, EntityType, ExportKind, ExportSection,
    FunctionSection, GlobalSection, ImportSection, MemorySection, MemoryType, Module, TableSection, TypeSection, ValType,
};
use wjsm_ir::Builtin;

use super::state::{CompileMode, Compiler, import_eval_global};
use crate::host_abi::HOST_IMPORT_NAMES;

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
        let mut imports = ImportSection::new();
        // Import index 0: console_log: (i64) -> ()
        imports.import("env", "console_log", EntityType::Function(0));
        // Import index 1: f64_mod: (i64, i64) -> (i64)
        imports.import("env", "f64_mod", EntityType::Function(2));
        // Import index 2: f64_pow: (i64, i64) -> (i64)
        imports.import("env", "f64_pow", EntityType::Function(2));
        // Import index 3: throw: (i64) -> ()
        imports.import("env", "throw", EntityType::Function(0));
        // Import index 4: iterator_from: (i64) -> (i64)
        imports.import("env", "iterator_from", EntityType::Function(3));
        // Import index 5: iterator_next: (i64) -> (i64)
        imports.import("env", "iterator_next", EntityType::Function(3));
        // Import index 6: iterator_close: (i64) -> ()
        imports.import("env", "iterator_close", EntityType::Function(0));
        // Import index 7: iterator_value: (i64) -> (i64)
        imports.import("env", "iterator_value", EntityType::Function(3));
        // Import index 8: iterator_done: (i64) -> (i64)
        imports.import("env", "iterator_done", EntityType::Function(3));
        // Import index 9: enumerator_from: (i64) -> (i64)
        imports.import("env", "enumerator_from", EntityType::Function(3));
        // Import index 10: enumerator_next: (i64) -> (i64)
        imports.import("env", "enumerator_next", EntityType::Function(3));
        // Import index 11: enumerator_key: (i64) -> (i64)
        imports.import("env", "enumerator_key", EntityType::Function(3));
        // Import index 12: enumerator_done: (i64) -> (i64)
        imports.import("env", "enumerator_done", EntityType::Function(3));
        // Import index 13: typeof: (i64) -> (i64)
        imports.import("env", "typeof", EntityType::Function(3));
        // Import index 14: op_in: (i64, i64) -> (i64)
        imports.import("env", "op_in", EntityType::Function(2));
        // Import index 15: op_instanceof: (i64, i64) -> (i64)
        imports.import("env", "op_instanceof", EntityType::Function(2));
        // Import index 16: string_concat: (i64, i64) -> (i64)
        imports.import("env", "string_concat", EntityType::Function(11));
        // Import index 17: string_concat_va: (i32, i32) -> (i64)
        imports.import("env", "string_concat_va", EntityType::Function(19));
        // Import index 18: define_property: (i64, i32, i64) -> ()
        imports.import("env", "define_property", EntityType::Function(9));
        // Import index 19: get_own_prop_desc: (i64, i32) -> (i64)
        imports.import("env", "get_own_prop_desc", EntityType::Function(8));
        // Import index 20: abstract_eq: (i64, i64) -> (i64)
        imports.import("env", "abstract_eq", EntityType::Function(2));
        // Import index 21: abstract_compare: (i64, i64) -> (i64)
        imports.import("env", "abstract_compare", EntityType::Function(2));
        // Import index 22: gc_collect: (i32) -> (i32)
        imports.import("env", "gc_collect", EntityType::Function(7)); // Type 7 = (i32) -> i32
        // Import index 23: console_error: (i64) -> ()
        imports.import("env", "console_error", EntityType::Function(0));
        // Import index 24: console_warn: (i64) -> ()
        imports.import("env", "console_warn", EntityType::Function(0));
        // Import index 25: console_info: (i64) -> ()
        imports.import("env", "console_info", EntityType::Function(0));
        // Import index 26: console_debug: (i64) -> ()
        imports.import("env", "console_debug", EntityType::Function(0));
        // Import index 27: console_trace: (i64) -> ()
        imports.import("env", "console_trace", EntityType::Function(0));
        // Import index 28: set_timeout: (i64, i64) -> (i64)
        imports.import("env", "set_timeout", EntityType::Function(2));
        // Import index 29: clear_timeout: (i64) -> ()
        imports.import("env", "clear_timeout", EntityType::Function(0));
        // Import index 30: set_interval: (i64, i64) -> (i64)
        imports.import("env", "set_interval", EntityType::Function(2));
        // Import index 31: clear_interval: (i64) -> ()
        imports.import("env", "clear_interval", EntityType::Function(0));
        // Import index 32: fetch: (i64) -> (i64)
        imports.import("env", "fetch", EntityType::Function(3));
        // Import index 33: json_stringify: (i64) -> (i64)
        imports.import("env", "json_stringify", EntityType::Function(3));
        // Import index 34: json_parse: (i64) -> (i64)
        imports.import("env", "json_parse", EntityType::Function(3));
        // Import index 35: closure_create: (i32, i64) -> (i64)
        imports.import("env", "closure_create", EntityType::Function(13));
        // Import index 36: closure_get_func: (i32) -> (i32)
        imports.import("env", "closure_get_func", EntityType::Function(14));
        // Import index 37: closure_get_env: (i32) -> (i64)
        imports.import("env", "closure_get_env", EntityType::Function(15));
        // Import index 38: arr_push: (i64, i64) -> (i64)
        imports.import("env", "arr_push", EntityType::Function(2));
        // Import index 39: arr_pop: (i64) -> (i64)
        imports.import("env", "arr_pop", EntityType::Function(3));
        // Import index 40: arr_includes: (i64, i64) -> (i64)
        imports.import("env", "arr_includes", EntityType::Function(2));
        // Import index 41: arr_index_of: (i64, i64, i64) -> (i64)
        imports.import("env", "arr_index_of", EntityType::Function(16));
        // Import index 42: arr_join: (i64, i64) -> (i64)
        imports.import("env", "arr_join", EntityType::Function(2));
        // Import index 43: arr_concat: (i64, i64) -> (i64)
        imports.import("env", "arr_concat", EntityType::Function(2));
        // Import index 44: arr_slice: (i64, i64, i64) -> (i64)
        imports.import("env", "arr_slice", EntityType::Function(16));
        // Import index 45: arr_fill: (i64, i64, i64, i64) -> (i64)
        imports.import("env", "arr_fill", EntityType::Function(17));
        // Import index 46: arr_reverse: (i64) -> (i64)
        imports.import("env", "arr_reverse", EntityType::Function(3));
        // Import index 47: arr_flat: (i64, i64) -> (i64)
        imports.import("env", "arr_flat", EntityType::Function(2));
        // Import index 48: arr_init_length: (i64, i64) -> (i64)
        imports.import("env", "arr_init_length", EntityType::Function(2));
        // Import index 49: arr_get_length: (i64) -> (i64)
        imports.import("env", "arr_get_length", EntityType::Function(3));
        // Import index 50: arr_proto_push: (i64, i64, i32, i32) -> (i64)
        imports.import("env", "arr_proto_push", EntityType::Function(12));
        // Import index 51: arr_proto_pop
        imports.import("env", "arr_proto_pop", EntityType::Function(12));
        // Import index 52: arr_proto_includes
        imports.import("env", "arr_proto_includes", EntityType::Function(12));
        // Import index 53: arr_proto_index_of
        imports.import("env", "arr_proto_index_of", EntityType::Function(12));
        // Import index 54: arr_proto_join
        imports.import("env", "arr_proto_join", EntityType::Function(12));
        // Import index 55: arr_proto_concat
        imports.import("env", "arr_proto_concat", EntityType::Function(12));
        // Import index 56: arr_proto_slice
        imports.import("env", "arr_proto_slice", EntityType::Function(12));
        // Import index 57: arr_proto_fill
        imports.import("env", "arr_proto_fill", EntityType::Function(12));
        // Import index 58: arr_proto_reverse
        imports.import("env", "arr_proto_reverse", EntityType::Function(12));
        // Import index 59: arr_proto_flat
        imports.import("env", "arr_proto_flat", EntityType::Function(12));
        // Import index 60: arr_proto_shift
        imports.import("env", "arr_proto_shift", EntityType::Function(12));
        // Import index 61: arr_proto_unshift
        imports.import("env", "arr_proto_unshift", EntityType::Function(12));
        // Import index 62: arr_proto_sort
        imports.import("env", "arr_proto_sort", EntityType::Function(12));
        // Import index 63: arr_proto_at
        imports.import("env", "arr_proto_at", EntityType::Function(12));
        // Import index 64: arr_proto_copy_within
        imports.import("env", "arr_proto_copy_within", EntityType::Function(12));
        // Import index 65: arr_proto_for_each
        imports.import("env", "arr_proto_for_each", EntityType::Function(12));
        // Import index 66: arr_proto_map
        imports.import("env", "arr_proto_map", EntityType::Function(12));
        // Import index 67: arr_proto_filter
        imports.import("env", "arr_proto_filter", EntityType::Function(12));
        // Import index 68: arr_proto_reduce
        imports.import("env", "arr_proto_reduce", EntityType::Function(12));
        // Import index 69: arr_proto_reduce_right
        imports.import("env", "arr_proto_reduce_right", EntityType::Function(12));
        // Import index 70: arr_proto_find
        imports.import("env", "arr_proto_find", EntityType::Function(12));
        // Import index 71: arr_proto_find_index
        imports.import("env", "arr_proto_find_index", EntityType::Function(12));
        // Import index 72: arr_proto_some
        imports.import("env", "arr_proto_some", EntityType::Function(12));
        // Import index 73: arr_proto_every
        imports.import("env", "arr_proto_every", EntityType::Function(12));
        // Import index 74: arr_proto_flat_map
        imports.import("env", "arr_proto_flat_map", EntityType::Function(12));
        // Import index 75: arr_proto_splice
        imports.import("env", "arr_proto_splice", EntityType::Function(12));
        // Import index 76: arr_proto_is_array
        imports.import("env", "arr_proto_is_array", EntityType::Function(12));
        // Import index 77: abort_shadow_stack_overflow: (i32, i32, i32) -> ()
        imports.import(
            "env",
            "abort_shadow_stack_overflow",
            EntityType::Function(18),
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
        types.ty().function(
            vec![ValType::I64, ValType::I64],
            vec![ValType::I64],
        );
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
        types.ty().function(
            vec![ValType::I64],
            vec![ValType::I64],
        );
        // Type 32: (i64, i32, i64) -> (i64) — private_set(obj, key_name_id, value)
        types.ty().function(
            vec![ValType::I64, ValType::I32, ValType::I64],
            vec![ValType::I64],
        );
        // Import index 78: func_call — Type 12 (uses shadow stack for args)
        imports.import("env", "func_call", EntityType::Function(12));
        // Import index 79: func_apply — Type 16 (i64 func, i64 this, i64 argsArray) -> i64
        imports.import("env", "func_apply", EntityType::Function(16));
        // Import index 80: func_bind — Type 12 (uses shadow stack for bound args)
        imports.import("env", "func_bind", EntityType::Function(12));
        // Import index 81: object_rest: (i64, i64) -> (i64)
        imports.import("env", "object_rest", EntityType::Function(2));
        // Import index 82: obj_spread: (i64, i64) -> ()
        imports.import("env", "obj_spread", EntityType::Function(5));
        // Import index 83: has_own_property: (i64, i32) -> (i64)
        imports.import("env", "has_own_property", EntityType::Function(8));
        // Import index 84: obj_keys: (i64) -> (i64)
        imports.import("env", "obj_keys", EntityType::Function(3));
        // Import index 85: obj_values: (i64) -> (i64)
        imports.import("env", "obj_values", EntityType::Function(3));
        // Import index 86: obj_entries: (i64) -> (i64)
        imports.import("env", "obj_entries", EntityType::Function(3));
        // Import index 87: obj_assign: (i64, i64, i32, i32) -> (i64)
        imports.import("env", "obj_assign", EntityType::Function(12));
        // Import index 88: obj_create: (i64, i64) -> (i64)
        imports.import("env", "obj_create", EntityType::Function(2));
        // Import index 89: obj_get_proto_of: (i64) -> (i64)
        imports.import("env", "obj_get_proto_of", EntityType::Function(3));
        // Import index 90: obj_set_proto_of: (i64, i64) -> (i64)
        imports.import("env", "obj_set_proto_of", EntityType::Function(2));
        // Import index 91: obj_get_own_prop_names: (i64) -> (i64)
        imports.import("env", "obj_get_own_prop_names", EntityType::Function(3));
        // Import index 92: obj_is: (i64, i64) -> (i64)
        imports.import("env", "obj_is", EntityType::Function(2));
        // Import index 93: obj_proto_to_string: (i64) -> (i64)
        imports.import("env", "obj_proto_to_string", EntityType::Function(3));
        // Import index 94: obj_proto_value_of: (i64) -> (i64)
        imports.import("env", "obj_proto_value_of", EntityType::Function(3));
        // Import index 95: bigint_from_literal: (i32, i32) -> i64
        imports.import("env", "bigint_from_literal", EntityType::Function(19));
        // Import index 96: bigint_add: (i64, i64) -> i64
        imports.import("env", "bigint_add", EntityType::Function(2));
        // Import index 97: bigint_sub: (i64, i64) -> i64
        imports.import("env", "bigint_sub", EntityType::Function(2));
        // Import index 98: bigint_mul: (i64, i64) -> i64
        imports.import("env", "bigint_mul", EntityType::Function(2));
        // Import index 99: bigint_div: (i64, i64) -> i64
        imports.import("env", "bigint_div", EntityType::Function(2));
        // Import index 100: bigint_mod: (i64, i64) -> i64
        imports.import("env", "bigint_mod", EntityType::Function(2));
        // Import index 101: bigint_pow: (i64, i64) -> i64
        imports.import("env", "bigint_pow", EntityType::Function(2));
        // Import index 102: bigint_neg: (i64) -> i64
        imports.import("env", "bigint_neg", EntityType::Function(3));
        // Import index 103: bigint_eq: (i64, i64) -> i64
        imports.import("env", "bigint_eq", EntityType::Function(2));
        // Import index 104: bigint_cmp: (i64, i64) -> i64
        imports.import("env", "bigint_cmp", EntityType::Function(2));
        // Import index 105: symbol_create: (i64) -> i64
        imports.import("env", "symbol_create", EntityType::Function(3));
        // Import index 106: symbol_for: (i64) -> i64
        imports.import("env", "symbol_for", EntityType::Function(3));
        // Import index 107: symbol_key_for: (i64) -> i64
        imports.import("env", "symbol_key_for", EntityType::Function(3));
        // Import index 108: symbol_well_known: (i32) -> i64
        imports.import("env", "symbol_well_known", EntityType::Function(15));
        // ── RegExp builtins ──
        // Import index 109: regex_create: (i32, i32, i32, i32) -> i64
        imports.import("env", "regex_create", EntityType::Function(20));
        // Import index 110: regex_test: (i64, i64) -> i64
        imports.import("env", "regex_test", EntityType::Function(2));
        // Import index 111: regex_exec: (i64, i64) -> i64
        imports.import("env", "regex_exec", EntityType::Function(2));
        // ── String prototype builtins ──
        // Import index 112: string_match: (i64, i64) -> i64
        imports.import("env", "string_match", EntityType::Function(2));
        // Import index 113: string_replace: (i64, i64, i64) -> i64
        imports.import("env", "string_replace", EntityType::Function(16));
        // Import index 114: string_search: (i64, i64) -> i64
        imports.import("env", "string_search", EntityType::Function(2));
        // Import index 115: string_split: (i64, i64, i64) -> i64
        imports.import("env", "string_split", EntityType::Function(16));
        // ── Promise / Async builtins ──
        // Import index 116: promise_create: (i64) -> i64
        imports.import("env", "promise_create", EntityType::Function(3));
        // Import index 117: promise_instance_resolve: (i64, i64) -> ()
        imports.import("env", "promise_instance_resolve", EntityType::Function(5));
        // Import index 118: promise_instance_reject: (i64, i64) -> ()
        imports.import("env", "promise_instance_reject", EntityType::Function(5));
        // Import index 119: promise_then: (i64, i64, i64) -> i64
        imports.import("env", "promise_then", EntityType::Function(16));
        // Import index 120: promise_catch: (i64, i64) -> i64
        imports.import("env", "promise_catch", EntityType::Function(2));
        // Import index 121: promise_finally: (i64, i64) -> i64
        imports.import("env", "promise_finally", EntityType::Function(2));
        // Import index 122: promise_all: (i64, i64) -> i64
        imports.import("env", "promise_all", EntityType::Function(2));
        // Import index 123: promise_race: (i64, i64) -> i64
        imports.import("env", "promise_race", EntityType::Function(2));
        // Import index 124: promise_all_settled: (i64, i64) -> i64
        imports.import("env", "promise_all_settled", EntityType::Function(2));
        // Import index 125: promise_any: (i64, i64) -> i64
        imports.import("env", "promise_any", EntityType::Function(2));
        // Import index 126: promise_resolve_static: (i64, i64) -> i64
        imports.import("env", "promise_resolve_static", EntityType::Function(2));
        // Import index 127: promise_reject_static: (i64, i64) -> i64
        imports.import("env", "promise_reject_static", EntityType::Function(2));
        // Import index 128: is_promise: (i64) -> i64
        imports.import("env", "is_promise", EntityType::Function(3));
        // Import index 129: queue_microtask: (i64) -> ()
        imports.import("env", "queue_microtask", EntityType::Function(0));
        // Import index 130: drain_microtasks: () -> ()
        imports.import("env", "drain_microtasks", EntityType::Function(1));
        // Import index 131: async_function_start: (i64) -> i64
        imports.import("env", "async_function_start", EntityType::Function(21));
        // Import index 132: async_function_resume: (i64, i64, i64, i64, i64) -> ()
        imports.import("env", "async_function_resume", EntityType::Function(22));
        // Import index 133: async_function_suspend: (i64, i64, i64) -> ()
        imports.import("env", "async_function_suspend", EntityType::Function(23));
        // Import index 134: continuation_create: (i64, i64, i64) -> i64
        imports.import("env", "continuation_create", EntityType::Function(24));
        // Import index 135: continuation_save_var: (i64, i64, i64) -> ()
        imports.import("env", "continuation_save_var", EntityType::Function(25));
        // Import index 136: continuation_load_var: (i64, i64) -> i64
        imports.import("env", "continuation_load_var", EntityType::Function(2));
        // Import index 137: async_generator_start: (i64) -> i64
        imports.import("env", "async_generator_start", EntityType::Function(3));
        // Import index 138: async_generator_next: (i64, i64) -> i64
        imports.import("env", "async_generator_next", EntityType::Function(2));
        // Import index 139: async_generator_return: (i64, i64) -> i64
        imports.import("env", "async_generator_return", EntityType::Function(2));
        // Import index 140: async_generator_throw: (i64, i64) -> i64
        imports.import("env", "async_generator_throw", EntityType::Function(2));
        // Import index 141: native_call: (i64 func, i64 this, i32 args_base, i32 args_count) -> i64
        imports.import("env", "native_call", EntityType::Function(12));
        // Import index 142: promise_create_resolve_function: (i64) -> i64
        imports.import(
            "env",
            "promise_create_resolve_function",
            EntityType::Function(3),
        );
        // Import index 143: promise_create_reject_function: (i64) -> i64
        imports.import(
            "env",
            "promise_create_reject_function",
            EntityType::Function(3),
        );
        // Import index 144: is_callable: (i64) -> i64
        imports.import("env", "is_callable", EntityType::Function(3));
        // Import index 145: promise_with_resolvers: (i64) -> i64
        imports.import("env", "promise_with_resolvers", EntityType::Function(3));
        // Import index 146: register_module_namespace: (i64, i64) -> ()
        imports.import("env", "register_module_namespace", EntityType::Function(5));
        // Import index 147: dynamic_import: (i64) -> i64
        imports.import("env", "dynamic_import", EntityType::Function(3));
        // Import index 148: eval_direct: (i64 code, i64 env) -> i64
        imports.import("env", "eval_direct", EntityType::Function(2));
        // Import index 149: eval_indirect: (i64 code) -> i64
        imports.import("env", "eval_indirect", EntityType::Function(3));

        // Import index 150: jsx_create_element: (i64 tag, i64 props, i64 children) -> i64
        imports.import("env", "jsx_create_element", EntityType::Function(27));
        // Import index 151: proxy_create: (i64 target, i64 handler) -> i64
        imports.import("env", "proxy_create", EntityType::Function(28));
        // Import index 152: proxy_revocable: (i64 target, i64 handler) -> i64
        imports.import("env", "proxy_revocable", EntityType::Function(28));
        // Import index 153: reflect_get: (i64 target, i64 prop, i64 receiver) -> i64
        imports.import("env", "reflect_get", EntityType::Function(29));
        // Import index 154: reflect_set: (i64 target, i64 prop, i64 value, i64 receiver) -> i64
        imports.import("env", "reflect_set", EntityType::Function(30));
        // Import index 155: reflect_has: (i64 target, i64 prop) -> i64
        imports.import("env", "reflect_has", EntityType::Function(28));
        // Import index 156: reflect_delete_property: (i64 target, i64 prop) -> i64
        imports.import("env", "reflect_delete_property", EntityType::Function(28));
        // Import index 157: reflect_apply: (i64 target, i64 this_arg, i64 args) -> i64
        imports.import("env", "reflect_apply", EntityType::Function(29));
        // Import index 158: reflect_construct: (i64 target, i64 args, i64 new_target) -> i64
        imports.import("env", "reflect_construct", EntityType::Function(29));
        // Import index 159: reflect_get_prototype_of: (i64 target) -> i64
        imports.import("env", "reflect_get_prototype_of", EntityType::Function(31));
        // Import index 160: reflect_set_prototype_of: (i64 target, i64 proto) -> i64
        imports.import("env", "reflect_set_prototype_of", EntityType::Function(28));
        // Import index 161: reflect_is_extensible: (i64 target) -> i64
        imports.import("env", "reflect_is_extensible", EntityType::Function(31));
        // Import index 162: reflect_prevent_extensions: (i64 target) -> i64
        imports.import("env", "reflect_prevent_extensions", EntityType::Function(31));
        // Import index 163: reflect_get_own_property_descriptor: (i64 target, i64 prop) -> i64
        imports.import("env", "reflect_get_own_property_descriptor", EntityType::Function(28));
        // Import index 164: reflect_define_property: (i64 target, i64 prop, i64 desc) -> i64
        imports.import("env", "reflect_define_property", EntityType::Function(29));
        // Import index 165: reflect_own_keys: (i64 target) -> i64
        imports.import("env", "reflect_own_keys", EntityType::Function(31));
        // Import index 166: string_at: (i64, i64) -> i64
        imports.import("env", "string_at", EntityType::Function(2));
        // Import 167: string_char_at: (i64, i64) -> i64
        imports.import("env", "string_char_at", EntityType::Function(2));
        // Import 168: string_char_code_at: (i64, i64) -> i64
        imports.import("env", "string_char_code_at", EntityType::Function(2));
        // Import 169: string_code_point_at: (i64, i64) -> i64
        imports.import("env", "string_code_point_at", EntityType::Function(2));
        // Import 170: string_proto_concat: (i64, i64, i32, i32) -> i64
        imports.import("env", "string_proto_concat", EntityType::Function(12));
        // Import 171: string_ends_with: (i64, i64, i64) -> i64
        imports.import("env", "string_ends_with", EntityType::Function(16));
        // Import 172: string_includes: (i64, i64, i64) -> i64
        imports.import("env", "string_includes", EntityType::Function(16));
        // Import 173: string_index_of: (i64, i64, i64) -> i64
        imports.import("env", "string_index_of", EntityType::Function(16));
        // Import 174: string_last_index_of: (i64, i64, i64) -> i64
        imports.import("env", "string_last_index_of", EntityType::Function(16));
        // Import 175: string_match_all: (i64, i64, i32, i32) -> i64
        imports.import("env", "string_match_all", EntityType::Function(12));
        // Import 176: string_pad_end: (i64, i64, i64) -> i64
        imports.import("env", "string_pad_end", EntityType::Function(16));
        // Import 177: string_pad_start: (i64, i64, i64) -> i64
        imports.import("env", "string_pad_start", EntityType::Function(16));
        // Import 178: string_repeat: (i64, i64) -> i64
        imports.import("env", "string_repeat", EntityType::Function(2));
        // Import 179: string_replace_all: (i64, i64, i64) -> i64
        imports.import("env", "string_replace_all", EntityType::Function(16));
        // Import 180: string_slice: (i64, i64, i64) -> i64
        imports.import("env", "string_slice", EntityType::Function(16));
        // Import 181: string_starts_with: (i64, i64, i64) -> i64
        imports.import("env", "string_starts_with", EntityType::Function(16));
        // Import 182: string_substring: (i64, i64, i64) -> i64
        imports.import("env", "string_substring", EntityType::Function(16));
        // Import 183: string_to_lower_case: (i64) -> i64
        imports.import("env", "string_to_lower_case", EntityType::Function(3));
        // Import 184: string_to_upper_case: (i64) -> i64
        imports.import("env", "string_to_upper_case", EntityType::Function(3));
        // Import 185: string_trim: (i64) -> i64
        imports.import("env", "string_trim", EntityType::Function(3));
        // Import 186: string_trim_end: (i64) -> i64
        imports.import("env", "string_trim_end", EntityType::Function(3));
        // Import 187: string_trim_start: (i64) -> i64
        imports.import("env", "string_trim_start", EntityType::Function(3));
        // Import 188: string_to_string: (i64) -> i64
        imports.import("env", "string_to_string", EntityType::Function(3));
        // Import 189: string_value_of: (i64) -> i64
        imports.import("env", "string_value_of", EntityType::Function(3));
        // Import 190: string_iterator: (i64) -> i64
        imports.import("env", "string_iterator", EntityType::Function(3));
        // Import 191: string_from_char_code: (i64, i64, i32, i32) -> i64
        imports.import("env", "string_from_char_code", EntityType::Function(12));
        // Import 192: string_from_code_point: (i64, i64, i32, i32) -> i64
        imports.import("env", "string_from_code_point", EntityType::Function(12));
        // ── Math builtins ──
        // Import index 193: math_abs: (i64) -> i64
        imports.import("env", "math_abs", EntityType::Function(3));
        // Import index 194: math_acos: (i64) -> i64
        imports.import("env", "math_acos", EntityType::Function(3));
        // Import index 195: math_acosh: (i64) -> i64
        imports.import("env", "math_acosh", EntityType::Function(3));
        // Import index 196: math_asin: (i64) -> i64
        imports.import("env", "math_asin", EntityType::Function(3));
        // Import index 197: math_asinh: (i64) -> i64
        imports.import("env", "math_asinh", EntityType::Function(3));
        // Import index 198: math_atan: (i64) -> i64
        imports.import("env", "math_atan", EntityType::Function(3));
        // Import index 199: math_atanh: (i64) -> i64
        imports.import("env", "math_atanh", EntityType::Function(3));
        // Import index 200: math_atan2: (i64, i64) -> i64
        imports.import("env", "math_atan2", EntityType::Function(2));
        // Import index 201: math_cbrt: (i64) -> i64
        imports.import("env", "math_cbrt", EntityType::Function(3));
        // Import index 202: math_ceil: (i64) -> i64
        imports.import("env", "math_ceil", EntityType::Function(3));
        // Import index 203: math_clz32: (i64) -> i64
        imports.import("env", "math_clz32", EntityType::Function(3));
        // Import index 204: math_cos: (i64) -> i64
        imports.import("env", "math_cos", EntityType::Function(3));
        // Import index 205: math_cosh: (i64) -> i64
        imports.import("env", "math_cosh", EntityType::Function(3));
        // Import index 206: math_exp: (i64) -> i64
        imports.import("env", "math_exp", EntityType::Function(3));
        // Import index 207: math_expm1: (i64) -> i64
        imports.import("env", "math_expm1", EntityType::Function(3));
        // Import index 208: math_floor: (i64) -> i64
        imports.import("env", "math_floor", EntityType::Function(3));
        // Import index 209: math_fround: (i64) -> i64
        imports.import("env", "math_fround", EntityType::Function(3));
        // Import index 210: math_hypot: (i32, i32) -> i64
        imports.import("env", "math_hypot", EntityType::Function(19));
        // Import index 211: math_imul: (i64, i64) -> i64
        imports.import("env", "math_imul", EntityType::Function(2));
        // Import index 212: math_log: (i64) -> i64
        imports.import("env", "math_log", EntityType::Function(3));
        // Import index 213: math_log1p: (i64) -> i64
        imports.import("env", "math_log1p", EntityType::Function(3));
        // Import index 214: math_log10: (i64) -> i64
        imports.import("env", "math_log10", EntityType::Function(3));
        // Import index 215: math_log2: (i64) -> i64
        imports.import("env", "math_log2", EntityType::Function(3));
        // Import index 216: math_max: (i32, i32) -> i64
        imports.import("env", "math_max", EntityType::Function(19));
        // Import index 217: math_min: (i32, i32) -> i64
        imports.import("env", "math_min", EntityType::Function(19));
        // Import index 218: math_pow: (i64, i64) -> i64
        imports.import("env", "math_pow", EntityType::Function(2));
        // Import index 219: math_random: () -> i64
        imports.import("env", "math_random", EntityType::Function(4));
        // Import index 220: math_round: (i64) -> i64
        imports.import("env", "math_round", EntityType::Function(3));
        // Import index 221: math_sign: (i64) -> i64
        imports.import("env", "math_sign", EntityType::Function(3));
        // Import index 222: math_sin: (i64) -> i64
        imports.import("env", "math_sin", EntityType::Function(3));
        // Import index 223: math_sinh: (i64) -> i64
        imports.import("env", "math_sinh", EntityType::Function(3));
        // Import index 224: math_sqrt: (i64) -> i64
        imports.import("env", "math_sqrt", EntityType::Function(3));
        // Import index 225: math_tan: (i64) -> i64
        imports.import("env", "math_tan", EntityType::Function(3));
        // Import index 226: math_tanh: (i64) -> i64
        imports.import("env", "math_tanh", EntityType::Function(3));
        // Import index 227: math_trunc: (i64) -> i64
        imports.import("env", "math_trunc", EntityType::Function(3));
        // ── Number imports ──
        // Import index 228: number_constructor: (i64) -> i64
        imports.import("env", "number_constructor", EntityType::Function(3));
        // Import index 229: number_is_nan: (i64) -> i64
        imports.import("env", "number_is_nan", EntityType::Function(3));
        // Import index 230: number_is_finite: (i64) -> i64
        imports.import("env", "number_is_finite", EntityType::Function(3));
        // Import index 231: number_is_integer: (i64) -> i64
        imports.import("env", "number_is_integer", EntityType::Function(3));
        // Import index 232: number_is_safe_integer: (i64) -> i64
        imports.import("env", "number_is_safe_integer", EntityType::Function(3));
        // Import index 233: number_parse_int: (i64, i64) -> i64
        imports.import("env", "number_parse_int", EntityType::Function(2));
        // Import index 234: number_parse_float: (i64) -> i64
        imports.import("env", "number_parse_float", EntityType::Function(3));
        // Import index 235: number_proto_to_string: (i64, i64) -> i64
        imports.import("env", "number_proto_to_string", EntityType::Function(2));
        // Import index 236: number_proto_value_of: (i64) -> i64
        imports.import("env", "number_proto_value_of", EntityType::Function(3));
        // Import index 237: number_proto_to_fixed: (i64, i64) -> i64
        imports.import("env", "number_proto_to_fixed", EntityType::Function(2));
        // Import index 238: number_proto_to_exponential: (i64, i64) -> i64
        imports.import("env", "number_proto_to_exponential", EntityType::Function(2));
        // Import index 239: number_proto_to_precision: (i64, i64) -> i64
        imports.import("env", "number_proto_to_precision", EntityType::Function(2));
        // ── Boolean imports ──
        // Import index 240: boolean_constructor: (i64) -> i64
        imports.import("env", "boolean_constructor", EntityType::Function(3));
        // Import index 241: boolean_proto_to_string: (i64) -> i64
        imports.import("env", "boolean_proto_to_string", EntityType::Function(3));
        // Import index 242: boolean_proto_value_of: (i64) -> i64
        imports.import("env", "boolean_proto_value_of", EntityType::Function(3));
        // ── Error imports ──
        // Import index 243: error_constructor: (i64) -> i64
        imports.import("env", "error_constructor", EntityType::Function(3));
        // Import index 244: type_error_constructor: (i64) -> i64
        imports.import("env", "type_error_constructor", EntityType::Function(3));
        // Import index 245: range_error_constructor: (i64) -> i64
        imports.import("env", "range_error_constructor", EntityType::Function(3));
        // Import index 246: syntax_error_constructor: (i64) -> i64
        imports.import("env", "syntax_error_constructor", EntityType::Function(3));
        // Import index 247: reference_error_constructor: (i64) -> i64
        imports.import("env", "reference_error_constructor", EntityType::Function(3));
        // Import index 248: uri_error_constructor: (i64) -> i64
        imports.import("env", "uri_error_constructor", EntityType::Function(3));
        // Import index 249: eval_error_constructor: (i64) -> i64
        imports.import("env", "eval_error_constructor", EntityType::Function(3));
        // Import index 250: error_proto_to_string: (i64) -> i64
        imports.import("env", "error_proto_to_string", EntityType::Function(3));
        // ── Map imports ──
        // Import index 251: map_constructor: (i64) -> i64
        imports.import("env", "map_constructor", EntityType::Function(3));
        // Import index 252: map_proto_set: (i64, i64, i64) -> i64
        imports.import("env", "map_proto_set", EntityType::Function(16));
        // Import index 253: map_proto_get: (i64, i64) -> i64
        imports.import("env", "map_proto_get", EntityType::Function(2));
        // ── Set imports ──
        // Import index 254: set_constructor: (i64) -> i64
        imports.import("env", "set_constructor", EntityType::Function(3));
        // Import index 255: set_proto_add: (i64, i64) -> i64
        imports.import("env", "set_proto_add", EntityType::Function(2));
        // ── Map/Set shared imports ──
        // Import index 256: map_set_has: (i64, i64) -> i64
        imports.import("env", "map_set_has", EntityType::Function(2));
        // Import index 257: map_set_delete: (i64, i64) -> i64
        imports.import("env", "map_set_delete", EntityType::Function(2));
        // Import index 258: map_set_clear: (i64) -> i64
        imports.import("env", "map_set_clear", EntityType::Function(3));
        imports.import("env", "map_set_get_size", EntityType::Function(3));
        // Import index 260: map_set_for_each: (i64) -> i64
        imports.import("env", "map_set_for_each", EntityType::Function(3));
        // Import index 261: map_set_keys: (i64) -> i64
        imports.import("env", "map_set_keys", EntityType::Function(3));
        // Import index 262: map_set_values: (i64) -> i64
        imports.import("env", "map_set_values", EntityType::Function(3));
        // Import index 263: map_set_entries: (i64) -> i64
        imports.import("env", "map_set_entries", EntityType::Function(3));
        // ── Date imports ──
        // Import index 264: date_constructor: Type 12 (shadow stack, variadic args)
        imports.import("env", "date_constructor", EntityType::Function(12));
        // Import index 265: date_now: () -> i64
        imports.import("env", "date_now", EntityType::Function(4));
        // Import index 266: date_parse: (i64) -> i64
        imports.import("env", "date_parse", EntityType::Function(3));
        // Import index 267: date_utc: (i64) -> i64
        imports.import("env", "date_utc", EntityType::Function(3));
        // ── WeakMap imports ──
        // Import index 268: weakmap_constructor: (i64) -> i64
        imports.import("env", "weakmap_constructor", EntityType::Function(3));
        // Import index 269: weakmap_proto_set: (i64, i64, i64) -> i64
        imports.import("env", "weakmap_proto_set", EntityType::Function(16));
        // Import index 270: weakmap_proto_get: (i64, i64) -> i64
        imports.import("env", "weakmap_proto_get", EntityType::Function(2));
        // Import index 271: weakmap_proto_has: (i64, i64) -> i64
        imports.import("env", "weakmap_proto_has", EntityType::Function(2));
        // Import index 272: weakmap_proto_delete: (i64, i64) -> i64
        imports.import("env", "weakmap_proto_delete", EntityType::Function(2));
        // ── WeakSet imports ──
        // Import index 273: weakset_constructor: (i64) -> i64
        imports.import("env", "weakset_constructor", EntityType::Function(3));
        // Import index 274: weakset_proto_add: (i64, i64) -> i64
        imports.import("env", "weakset_proto_add", EntityType::Function(2));
        // Import index 275: weakset_proto_has: (i64, i64) -> i64
        imports.import("env", "weakset_proto_has", EntityType::Function(2));
        // Import index 276: weakset_proto_delete: (i64, i64) -> i64
        imports.import("env", "weakset_proto_delete", EntityType::Function(2));
        // ── ArrayBuffer imports ──
        // Import index 277: arraybuffer_constructor: (i64) -> i64
        imports.import("env", "arraybuffer_constructor", EntityType::Function(3));
        // Import index 278: arraybuffer_proto_byte_length: (i64) -> i64
        imports.import("env", "arraybuffer_proto_byte_length", EntityType::Function(3));
        // Import index 279: arraybuffer_proto_slice: (i64, i64, i64) -> i64
        imports.import("env", "arraybuffer_proto_slice", EntityType::Function(16));
        // ── DataView imports ──
        // Import index 280: dataview_constructor: (i64, i64, i64) -> i64
        imports.import("env", "dataview_constructor", EntityType::Function(16));
        // Import index 281-288: DataView get methods: (i64, i64) -> i64
        imports.import("env", "dataview_proto_get_float64", EntityType::Function(2));
        imports.import("env", "dataview_proto_get_float32", EntityType::Function(2));
        imports.import("env", "dataview_proto_get_int32", EntityType::Function(2));
        imports.import("env", "dataview_proto_get_uint32", EntityType::Function(2));
        imports.import("env", "dataview_proto_get_int16", EntityType::Function(2));
        imports.import("env", "dataview_proto_get_uint16", EntityType::Function(2));
        imports.import("env", "dataview_proto_get_int8", EntityType::Function(2));
        imports.import("env", "dataview_proto_get_uint8", EntityType::Function(2));
        // Import index 289-296: DataView set methods: (i64, i64, i64) -> i64
        imports.import("env", "dataview_proto_set_float64", EntityType::Function(16));
        imports.import("env", "dataview_proto_set_float32", EntityType::Function(16));
        imports.import("env", "dataview_proto_set_int32", EntityType::Function(16));
        imports.import("env", "dataview_proto_set_uint32", EntityType::Function(16));
        imports.import("env", "dataview_proto_set_int16", EntityType::Function(16));
        imports.import("env", "dataview_proto_set_uint16", EntityType::Function(16));
        imports.import("env", "dataview_proto_set_int8", EntityType::Function(16));
        imports.import("env", "dataview_proto_set_uint8", EntityType::Function(16));
        // ── TypedArray constructor imports ──
        // Import index 297-305: TypedArray constructors: (i64, i64, i64) -> i64
        imports.import("env", "int8array_constructor", EntityType::Function(16));
        imports.import("env", "uint8array_constructor", EntityType::Function(16));
        imports.import("env", "uint8clampedarray_constructor", EntityType::Function(16));
        imports.import("env", "int16array_constructor", EntityType::Function(16));
        imports.import("env", "uint16array_constructor", EntityType::Function(16));
        imports.import("env", "int32array_constructor", EntityType::Function(16));
        imports.import("env", "uint32array_constructor", EntityType::Function(16));
        imports.import("env", "float32array_constructor", EntityType::Function(16));
        imports.import("env", "float64array_constructor", EntityType::Function(16));
        // ── TypedArray prototype imports ──
        // Import index 306: typedarray_proto_length: (i64) -> i64
        imports.import("env", "typedarray_proto_length", EntityType::Function(3));
        // Import index 307: typedarray_proto_byte_length: (i64) -> i64
        imports.import("env", "typedarray_proto_byte_length", EntityType::Function(3));
        // Import index 308: typedarray_proto_byte_offset: (i64) -> i64
        imports.import("env", "typedarray_proto_byte_offset", EntityType::Function(3));
        // Import index 309: typedarray_proto_set: (i64, i64, i64) -> i64
        imports.import("env", "typedarray_proto_set", EntityType::Function(16));
        // Import index 310: typedarray_proto_slice: (i64, i64, i64) -> i64
        imports.import("env", "typedarray_proto_slice", EntityType::Function(16));
        // Import index 311: typedarray_proto_subarray: (i64, i64, i64) -> i64
        imports.import("env", "typedarray_proto_subarray", EntityType::Function(16));
        // Import index 312: get_builtin_global: (i64) -> i64
        imports.import("env", "get_builtin_global", EntityType::Function(3));
        // Import index 313: private_get: (i64, i32) -> i64
        imports.import("env", "private_get", EntityType::Function(8));
        // Import index 314: private_set: (i64, i32, i64) -> i64
        imports.import("env", "private_set", EntityType::Function(32));
        // Import index 315: private_has: (i64, i32) -> i64
        imports.import("env", "private_has", EntityType::Function(8));
        if mode == CompileMode::Eval {
            imports.import(
                "env",
                "memory",
                EntityType::Memory(MemoryType {
                    minimum: 2,
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
        }
        let mut builtin_func_indices = HashMap::new();
        builtin_func_indices.insert(Builtin::ConsoleLog, 0);
        builtin_func_indices.insert(Builtin::ConsoleError, 23);
        builtin_func_indices.insert(Builtin::ConsoleWarn, 24);
        builtin_func_indices.insert(Builtin::ConsoleInfo, 25);
        builtin_func_indices.insert(Builtin::ConsoleDebug, 26);
        builtin_func_indices.insert(Builtin::ConsoleTrace, 27);
        builtin_func_indices.insert(Builtin::F64Mod, 1);
        builtin_func_indices.insert(Builtin::F64Exp, 2);
        builtin_func_indices.insert(Builtin::Throw, 3);
        builtin_func_indices.insert(Builtin::AbortShadowStackOverflow, 77);
        builtin_func_indices.insert(Builtin::IteratorFrom, 4);
        builtin_func_indices.insert(Builtin::IteratorNext, 5);
        builtin_func_indices.insert(Builtin::IteratorClose, 6);
        builtin_func_indices.insert(Builtin::IteratorValue, 7);
        builtin_func_indices.insert(Builtin::IteratorDone, 8);
        builtin_func_indices.insert(Builtin::EnumeratorFrom, 9);
        builtin_func_indices.insert(Builtin::EnumeratorNext, 10);
        builtin_func_indices.insert(Builtin::EnumeratorKey, 11);
        builtin_func_indices.insert(Builtin::EnumeratorDone, 12);
        builtin_func_indices.insert(Builtin::TypeOf, 13);
        builtin_func_indices.insert(Builtin::In, 14);
        builtin_func_indices.insert(Builtin::InstanceOf, 15);
        builtin_func_indices.insert(Builtin::DefineProperty, 18);
        builtin_func_indices.insert(Builtin::GetOwnPropDesc, 19);
        builtin_func_indices.insert(Builtin::AbstractEq, 20);
        builtin_func_indices.insert(Builtin::AbstractCompare, 21);
        builtin_func_indices.insert(Builtin::SetTimeout, 28);
        builtin_func_indices.insert(Builtin::ClearTimeout, 29);
        builtin_func_indices.insert(Builtin::SetInterval, 30);
        builtin_func_indices.insert(Builtin::ClearInterval, 31);
        builtin_func_indices.insert(Builtin::Fetch, 32);
        builtin_func_indices.insert(Builtin::JsonStringify, 33);
        builtin_func_indices.insert(Builtin::JsonParse, 34);
        builtin_func_indices.insert(Builtin::ArrayPush, 38);
        builtin_func_indices.insert(Builtin::ArrayPop, 39);
        builtin_func_indices.insert(Builtin::ArrayIncludes, 40);
        builtin_func_indices.insert(Builtin::ArrayIndexOf, 41);
        builtin_func_indices.insert(Builtin::ArrayJoin, 42);
        builtin_func_indices.insert(Builtin::ArrayConcat, 43);
        builtin_func_indices.insert(Builtin::ArraySlice, 44);
        builtin_func_indices.insert(Builtin::ArrayFill, 45);
        builtin_func_indices.insert(Builtin::ArrayReverse, 46);
        builtin_func_indices.insert(Builtin::ArrayFlat, 59);
        builtin_func_indices.insert(Builtin::ArrayInitLength, 48);
        builtin_func_indices.insert(Builtin::ArrayGetLength, 49);
        builtin_func_indices.insert(Builtin::ArrayShift, 60);
        builtin_func_indices.insert(Builtin::ArrayUnshiftVa, 61);
        builtin_func_indices.insert(Builtin::ArraySort, 62);
        builtin_func_indices.insert(Builtin::ArrayAt, 63);
        builtin_func_indices.insert(Builtin::ArrayCopyWithin, 64);
        builtin_func_indices.insert(Builtin::ArrayForEach, 65);
        builtin_func_indices.insert(Builtin::ArrayMap, 66);
        builtin_func_indices.insert(Builtin::ArrayFilter, 67);
        builtin_func_indices.insert(Builtin::ArrayReduce, 68);
        builtin_func_indices.insert(Builtin::ArrayReduceRight, 69);
        builtin_func_indices.insert(Builtin::ArrayFind, 70);
        builtin_func_indices.insert(Builtin::ArrayFindIndex, 71);
        builtin_func_indices.insert(Builtin::ArraySome, 72);
        builtin_func_indices.insert(Builtin::ArrayEvery, 73);
        builtin_func_indices.insert(Builtin::ArrayFlatMap, 74);
        builtin_func_indices.insert(Builtin::ArraySpliceVa, 75);
        builtin_func_indices.insert(Builtin::ArrayIsArray, 76);
        builtin_func_indices.insert(Builtin::ArrayConcatVa, 55);
        builtin_func_indices.insert(Builtin::FuncCall, 78);
        builtin_func_indices.insert(Builtin::FuncApply, 79);
        builtin_func_indices.insert(Builtin::FuncBind, 80);
        builtin_func_indices.insert(Builtin::ObjectRest, 81);
        builtin_func_indices.insert(Builtin::HasOwnProperty, 83);
        builtin_func_indices.insert(Builtin::ObjectKeys, 84);
        builtin_func_indices.insert(Builtin::ObjectValues, 85);
        builtin_func_indices.insert(Builtin::ObjectEntries, 86);
        builtin_func_indices.insert(Builtin::ObjectAssign, 87);
        builtin_func_indices.insert(Builtin::ObjectCreate, 88);
        builtin_func_indices.insert(Builtin::ObjectGetPrototypeOf, 89);
        builtin_func_indices.insert(Builtin::ObjectSetPrototypeOf, 90);
        builtin_func_indices.insert(Builtin::ObjectGetOwnPropertyNames, 91);
        builtin_func_indices.insert(Builtin::ObjectIs, 92);
        builtin_func_indices.insert(Builtin::ObjectProtoToString, 93);
        builtin_func_indices.insert(Builtin::ObjectProtoValueOf, 94);
        // ── BigInt builtins ──
        builtin_func_indices.insert(Builtin::BigIntFromLiteral, 95);
        builtin_func_indices.insert(Builtin::BigIntAdd, 96);
        builtin_func_indices.insert(Builtin::BigIntSub, 97);
        builtin_func_indices.insert(Builtin::BigIntMul, 98);
        builtin_func_indices.insert(Builtin::BigIntDiv, 99);
        builtin_func_indices.insert(Builtin::BigIntMod, 100);
        builtin_func_indices.insert(Builtin::BigIntPow, 101);
        builtin_func_indices.insert(Builtin::BigIntNeg, 102);
        builtin_func_indices.insert(Builtin::BigIntEq, 103);
        builtin_func_indices.insert(Builtin::BigIntCmp, 104);
        // ── Symbol builtins ──
        builtin_func_indices.insert(Builtin::SymbolCreate, 105);
        builtin_func_indices.insert(Builtin::SymbolFor, 106);
        builtin_func_indices.insert(Builtin::SymbolKeyFor, 107);
        builtin_func_indices.insert(Builtin::SymbolWellKnown, 108);
        // ── RegExp builtins ──
        builtin_func_indices.insert(Builtin::RegExpCreate, 109);
        builtin_func_indices.insert(Builtin::RegExpTest, 110);
        builtin_func_indices.insert(Builtin::RegExpExec, 111);
        // ── String prototype builtins ──
        builtin_func_indices.insert(Builtin::StringMatch, 112);
        builtin_func_indices.insert(Builtin::StringReplace, 113);
        builtin_func_indices.insert(Builtin::StringSearch, 114);
        builtin_func_indices.insert(Builtin::StringSplit, 115);
        // ── Promise / Async builtins ──
        builtin_func_indices.insert(Builtin::PromiseCreate, 116);
        builtin_func_indices.insert(Builtin::PromiseInstanceResolve, 117);
        builtin_func_indices.insert(Builtin::PromiseInstanceReject, 118);
        builtin_func_indices.insert(Builtin::PromiseCreateResolveFunction, 142);
        builtin_func_indices.insert(Builtin::PromiseCreateRejectFunction, 143);
        builtin_func_indices.insert(Builtin::PromiseThen, 119);
        builtin_func_indices.insert(Builtin::PromiseCatch, 120);
        builtin_func_indices.insert(Builtin::PromiseFinally, 121);
        builtin_func_indices.insert(Builtin::PromiseAll, 122);
        builtin_func_indices.insert(Builtin::PromiseRace, 123);
        builtin_func_indices.insert(Builtin::PromiseAllSettled, 124);
        builtin_func_indices.insert(Builtin::PromiseAny, 125);
        builtin_func_indices.insert(Builtin::PromiseResolveStatic, 126);
        builtin_func_indices.insert(Builtin::PromiseRejectStatic, 127);
        builtin_func_indices.insert(Builtin::IsPromise, 128);
        builtin_func_indices.insert(Builtin::QueueMicrotask, 129);
        builtin_func_indices.insert(Builtin::DrainMicrotasks, 130);
        builtin_func_indices.insert(Builtin::AsyncFunctionStart, 131);
        builtin_func_indices.insert(Builtin::AsyncFunctionResume, 132);
        builtin_func_indices.insert(Builtin::AsyncFunctionSuspend, 133);
        builtin_func_indices.insert(Builtin::ContinuationCreate, 134);
        builtin_func_indices.insert(Builtin::ContinuationSaveVar, 135);
        builtin_func_indices.insert(Builtin::ContinuationLoadVar, 136);
        builtin_func_indices.insert(Builtin::AsyncGeneratorStart, 137);
        builtin_func_indices.insert(Builtin::AsyncGeneratorNext, 138);
        builtin_func_indices.insert(Builtin::AsyncGeneratorReturn, 139);
        builtin_func_indices.insert(Builtin::AsyncGeneratorThrow, 140);
        builtin_func_indices.insert(Builtin::PromiseWithResolvers, 145);
        builtin_func_indices.insert(Builtin::IsCallable, 144);
        // ── 动态 import builtins ──
        builtin_func_indices.insert(Builtin::RegisterModuleNamespace, 146);
        builtin_func_indices.insert(Builtin::DynamicImport, 147);
        builtin_func_indices.insert(Builtin::Eval, 148);
        builtin_func_indices.insert(Builtin::EvalIndirect, 149);
        builtin_func_indices.insert(Builtin::JsxCreateElement, 150);
        builtin_func_indices.insert(Builtin::ProxyCreate, 151);
        builtin_func_indices.insert(Builtin::ProxyRevocable, 152);
        builtin_func_indices.insert(Builtin::ReflectGet, 153);
        builtin_func_indices.insert(Builtin::ReflectSet, 154);
        builtin_func_indices.insert(Builtin::ReflectHas, 155);
        builtin_func_indices.insert(Builtin::ReflectDeleteProperty, 156);
        builtin_func_indices.insert(Builtin::ReflectApply, 157);
        builtin_func_indices.insert(Builtin::ReflectConstruct, 158);
        builtin_func_indices.insert(Builtin::ReflectGetPrototypeOf, 159);
        builtin_func_indices.insert(Builtin::ReflectSetPrototypeOf, 160);
        builtin_func_indices.insert(Builtin::ReflectIsExtensible, 161);
        builtin_func_indices.insert(Builtin::ReflectPreventExtensions, 162);
        builtin_func_indices.insert(Builtin::ReflectGetOwnPropertyDescriptor, 163);
        builtin_func_indices.insert(Builtin::ReflectDefineProperty, 164);
        builtin_func_indices.insert(Builtin::ReflectOwnKeys, 165);
        builtin_func_indices.insert(Builtin::StringAt, 166);
        builtin_func_indices.insert(Builtin::StringCharAt, 167);
        builtin_func_indices.insert(Builtin::StringCharCodeAt, 168);
        builtin_func_indices.insert(Builtin::StringCodePointAt, 169);
        builtin_func_indices.insert(Builtin::StringConcatVa, 170);
        builtin_func_indices.insert(Builtin::StringEndsWith, 171);
        builtin_func_indices.insert(Builtin::StringIncludes, 172);
        builtin_func_indices.insert(Builtin::StringIndexOf, 173);
        builtin_func_indices.insert(Builtin::StringLastIndexOf, 174);
        builtin_func_indices.insert(Builtin::StringMatchAll, 175);
        builtin_func_indices.insert(Builtin::StringPadEnd, 176);
        builtin_func_indices.insert(Builtin::StringPadStart, 177);
        builtin_func_indices.insert(Builtin::StringRepeat, 178);
        builtin_func_indices.insert(Builtin::StringReplaceAll, 179);
        builtin_func_indices.insert(Builtin::StringSlice, 180);
        builtin_func_indices.insert(Builtin::StringStartsWith, 181);
        builtin_func_indices.insert(Builtin::StringSubstring, 182);
        builtin_func_indices.insert(Builtin::StringToLowerCase, 183);
        builtin_func_indices.insert(Builtin::StringToUpperCase, 184);
        builtin_func_indices.insert(Builtin::StringTrim, 185);
        builtin_func_indices.insert(Builtin::StringTrimEnd, 186);
        builtin_func_indices.insert(Builtin::StringTrimStart, 187);
        builtin_func_indices.insert(Builtin::StringToString, 188);
        builtin_func_indices.insert(Builtin::StringValueOf, 189);
        builtin_func_indices.insert(Builtin::StringIterator, 190);
        builtin_func_indices.insert(Builtin::StringFromCharCode, 191);
        builtin_func_indices.insert(Builtin::StringFromCodePoint, 192);
        // ── Math builtins ──
        builtin_func_indices.insert(Builtin::MathAbs, 193);
        builtin_func_indices.insert(Builtin::MathAcos, 194);
        builtin_func_indices.insert(Builtin::MathAcosh, 195);
        builtin_func_indices.insert(Builtin::MathAsin, 196);
        builtin_func_indices.insert(Builtin::MathAsinh, 197);
        builtin_func_indices.insert(Builtin::MathAtan, 198);
        builtin_func_indices.insert(Builtin::MathAtanh, 199);
        builtin_func_indices.insert(Builtin::MathAtan2, 200);
        builtin_func_indices.insert(Builtin::MathCbrt, 201);
        builtin_func_indices.insert(Builtin::MathCeil, 202);
        builtin_func_indices.insert(Builtin::MathClz32, 203);
        builtin_func_indices.insert(Builtin::MathCos, 204);
        builtin_func_indices.insert(Builtin::MathCosh, 205);
        builtin_func_indices.insert(Builtin::MathExp, 206);
        builtin_func_indices.insert(Builtin::MathExpm1, 207);
        builtin_func_indices.insert(Builtin::MathFloor, 208);
        builtin_func_indices.insert(Builtin::MathFround, 209);
        builtin_func_indices.insert(Builtin::MathHypot, 210);
        builtin_func_indices.insert(Builtin::MathImul, 211);
        builtin_func_indices.insert(Builtin::MathLog, 212);
        builtin_func_indices.insert(Builtin::MathLog1p, 213);
        builtin_func_indices.insert(Builtin::MathLog10, 214);
        builtin_func_indices.insert(Builtin::MathLog2, 215);
        builtin_func_indices.insert(Builtin::MathMax, 216);
        builtin_func_indices.insert(Builtin::MathMin, 217);
        builtin_func_indices.insert(Builtin::MathPow, 218);
        builtin_func_indices.insert(Builtin::MathRandom, 219);
        builtin_func_indices.insert(Builtin::MathRound, 220);
        builtin_func_indices.insert(Builtin::MathSign, 221);
        builtin_func_indices.insert(Builtin::MathSin, 222);
        builtin_func_indices.insert(Builtin::MathSinh, 223);
        builtin_func_indices.insert(Builtin::MathSqrt, 224);
        builtin_func_indices.insert(Builtin::MathTan, 225);
        builtin_func_indices.insert(Builtin::MathTanh, 226);
        builtin_func_indices.insert(Builtin::MathTrunc, 227);
        // ── Number builtins ──
        builtin_func_indices.insert(Builtin::NumberConstructor, 228);
        builtin_func_indices.insert(Builtin::NumberIsNaN, 229);
        builtin_func_indices.insert(Builtin::NumberIsFinite, 230);
        builtin_func_indices.insert(Builtin::NumberIsInteger, 231);
        builtin_func_indices.insert(Builtin::NumberIsSafeInteger, 232);
        builtin_func_indices.insert(Builtin::NumberParseInt, 233);
        builtin_func_indices.insert(Builtin::NumberParseFloat, 234);
        builtin_func_indices.insert(Builtin::NumberProtoToString, 235);
        builtin_func_indices.insert(Builtin::NumberProtoValueOf, 236);
        builtin_func_indices.insert(Builtin::NumberProtoToFixed, 237);
        builtin_func_indices.insert(Builtin::NumberProtoToExponential, 238);
        builtin_func_indices.insert(Builtin::NumberProtoToPrecision, 239);
        // ── Boolean builtins ──
        builtin_func_indices.insert(Builtin::BooleanConstructor, 240);
        builtin_func_indices.insert(Builtin::BooleanProtoToString, 241);
        builtin_func_indices.insert(Builtin::BooleanProtoValueOf, 242);
        // ── Error builtins ──
        builtin_func_indices.insert(Builtin::ErrorConstructor, 243);
        builtin_func_indices.insert(Builtin::TypeErrorConstructor, 244);
        builtin_func_indices.insert(Builtin::RangeErrorConstructor, 245);
        builtin_func_indices.insert(Builtin::SyntaxErrorConstructor, 246);
        builtin_func_indices.insert(Builtin::ReferenceErrorConstructor, 247);
        builtin_func_indices.insert(Builtin::URIErrorConstructor, 248);
        builtin_func_indices.insert(Builtin::EvalErrorConstructor, 249);
        builtin_func_indices.insert(Builtin::ErrorProtoToString, 250);
        // ── Map builtins ──
        builtin_func_indices.insert(Builtin::MapConstructor, 251);
        builtin_func_indices.insert(Builtin::MapProtoSet, 252);
        builtin_func_indices.insert(Builtin::MapProtoGet, 253);
        // ── Set builtins ──
        builtin_func_indices.insert(Builtin::SetConstructor, 254);
        builtin_func_indices.insert(Builtin::SetProtoAdd, 255);
        // ── Map/Set shared builtins ──
        builtin_func_indices.insert(Builtin::MapSetHas, 256);
        builtin_func_indices.insert(Builtin::MapSetDelete, 257);
        builtin_func_indices.insert(Builtin::MapSetClear, 258);
        builtin_func_indices.insert(Builtin::MapSetGetSize, 259);
        builtin_func_indices.insert(Builtin::MapSetForEach, 260);
        builtin_func_indices.insert(Builtin::MapSetKeys, 261);
        builtin_func_indices.insert(Builtin::MapSetValues, 262);
        builtin_func_indices.insert(Builtin::MapSetEntries, 263);
        // ── Date builtins ──
        builtin_func_indices.insert(Builtin::DateConstructor, 264);
        builtin_func_indices.insert(Builtin::DateNow, 265);
        builtin_func_indices.insert(Builtin::DateParse, 266);
        builtin_func_indices.insert(Builtin::DateUTC, 267);
        // ── WeakMap builtins ──
        builtin_func_indices.insert(Builtin::WeakMapConstructor, 268);
        builtin_func_indices.insert(Builtin::WeakMapProtoSet, 269);
        builtin_func_indices.insert(Builtin::WeakMapProtoGet, 270);
        builtin_func_indices.insert(Builtin::WeakMapProtoHas, 271);
        builtin_func_indices.insert(Builtin::WeakMapProtoDelete, 272);
        // ── WeakSet builtins ──
        builtin_func_indices.insert(Builtin::WeakSetConstructor, 273);
        builtin_func_indices.insert(Builtin::WeakSetProtoAdd, 274);
        builtin_func_indices.insert(Builtin::WeakSetProtoHas, 275);
        builtin_func_indices.insert(Builtin::WeakSetProtoDelete, 276);
        // ── ArrayBuffer builtins ──
        builtin_func_indices.insert(Builtin::ArrayBufferConstructor, 277);
        builtin_func_indices.insert(Builtin::ArrayBufferProtoByteLength, 278);
        builtin_func_indices.insert(Builtin::ArrayBufferProtoSlice, 279);
        // ── DataView builtins ──
        builtin_func_indices.insert(Builtin::DataViewConstructor, 280);
        builtin_func_indices.insert(Builtin::DataViewProtoGetFloat64, 281);
        builtin_func_indices.insert(Builtin::DataViewProtoGetFloat32, 282);
        builtin_func_indices.insert(Builtin::DataViewProtoGetInt32, 283);
        builtin_func_indices.insert(Builtin::DataViewProtoGetUint32, 284);
        builtin_func_indices.insert(Builtin::DataViewProtoGetInt16, 285);
        builtin_func_indices.insert(Builtin::DataViewProtoGetUint16, 286);
        builtin_func_indices.insert(Builtin::DataViewProtoGetInt8, 287);
        builtin_func_indices.insert(Builtin::DataViewProtoGetUint8, 288);
        builtin_func_indices.insert(Builtin::DataViewProtoSetFloat64, 289);
        builtin_func_indices.insert(Builtin::DataViewProtoSetFloat32, 290);
        builtin_func_indices.insert(Builtin::DataViewProtoSetInt32, 291);
        builtin_func_indices.insert(Builtin::DataViewProtoSetUint32, 292);
        builtin_func_indices.insert(Builtin::DataViewProtoSetInt16, 293);
        builtin_func_indices.insert(Builtin::DataViewProtoSetUint16, 294);
        builtin_func_indices.insert(Builtin::DataViewProtoSetInt8, 295);
        builtin_func_indices.insert(Builtin::DataViewProtoSetUint8, 296);
        // ── TypedArray constructor builtins ──
        builtin_func_indices.insert(Builtin::Int8ArrayConstructor, 297);
        builtin_func_indices.insert(Builtin::Uint8ArrayConstructor, 298);
        builtin_func_indices.insert(Builtin::Uint8ClampedArrayConstructor, 299);
        builtin_func_indices.insert(Builtin::Int16ArrayConstructor, 300);
        builtin_func_indices.insert(Builtin::Uint16ArrayConstructor, 301);
        builtin_func_indices.insert(Builtin::Int32ArrayConstructor, 302);
        builtin_func_indices.insert(Builtin::Uint32ArrayConstructor, 303);
        builtin_func_indices.insert(Builtin::Float32ArrayConstructor, 304);
        builtin_func_indices.insert(Builtin::Float64ArrayConstructor, 305);
        // ── TypedArray prototype builtins ──
        builtin_func_indices.insert(Builtin::TypedArrayProtoLength, 306);
        builtin_func_indices.insert(Builtin::TypedArrayProtoByteLength, 307);
        builtin_func_indices.insert(Builtin::TypedArrayProtoByteOffset, 308);
        builtin_func_indices.insert(Builtin::TypedArrayProtoSet, 309);
        builtin_func_indices.insert(Builtin::TypedArrayProtoSlice, 310);
        builtin_func_indices.insert(Builtin::TypedArrayProtoSubarray, 311);
        builtin_func_indices.insert(Builtin::GetBuiltinGlobal, 312);
        builtin_func_indices.insert(Builtin::PrivateGet, 313);
        builtin_func_indices.insert(Builtin::PrivateSet, 314);
        builtin_func_indices.insert(Builtin::PrivateHas, 315);
        let functions = FunctionSection::new();

        let mut exports = ExportSection::new();
        exports.export("memory", ExportKind::Memory, 0);
        for (index, name) in HOST_IMPORT_NAMES.iter().enumerate() {
            exports.export(name, ExportKind::Func, index as u32);
        }

        let mut memory = MemorySection::new();
        if mode == CompileMode::Normal {
            memory.memory(MemoryType {
                minimum: 2, // 2 pages (128KB) to accommodate shadow stack
                maximum: None,
                memory64: false,
                shared: false,
                page_size_log2: None,
            });
        }

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
            _next_import_func: HOST_IMPORT_NAMES.len() as u32,
            builtin_func_indices,
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
            eval_var_base_local_idx: 0,
            gc_collect_func_idx: 22,
            alloc_counter_global_idx: 0,
            object_heap_start_global_idx: 6,
            num_ir_functions_global_idx: 7,
            shadow_stack_end_global_idx: 8,
            closure_create_func_idx: 35,
            closure_get_func_idx: 36,
            closure_get_env_idx: 37,
            native_call_func_idx: 141,
            array_proto_handle_global_idx: 0,
            arr_proto_table_base: 0,
            obj_spread_func_idx: 0,
            get_proto_from_ctor_func_idx: 0,
            string_eq_func_idx: 0,
            object_proto_handle_global_idx: 0,
            eval_var_map_ptr_global_idx: 11,
            eval_var_map_count_global_idx: 12,
            eval_var_map_records: Vec::new(),
            eval_var_map_ptr: 0,
            eval_var_map_count: 0,
            continuation_local_idx: 0,
            current_function_has_eval: false,
            mode,
            function_param_counts: Vec::new(),
            function_names: Vec::new(),
        }
    }
}
