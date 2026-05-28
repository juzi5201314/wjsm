use super::*;

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
        // Import index 0: console_log: (i32, i32) -> ()
        imports.import("env", "console_log", EntityType::Function(33));
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
        // Import index 23: console_error: (i32, i32) -> ()
        imports.import("env", "console_error", EntityType::Function(33));
        // Import index 24: console_warn: (i32, i32) -> ()
        imports.import("env", "console_warn", EntityType::Function(33));
        // Import index 25: console_info: (i32, i32) -> ()
        imports.import("env", "console_info", EntityType::Function(33));
        // Import index 26: console_debug: (i32, i32) -> ()
        imports.import("env", "console_debug", EntityType::Function(33));
        // Import index 27: console_trace: (i32, i32) -> ()
        imports.import("env", "console_trace", EntityType::Function(33));
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
        // Import index 78: func_call — Type 12 (uses shadow stack for args)
        imports.import("env", "func_call", EntityType::Function(12));
        // Import index 79: func_apply — Type 16 (i64 func, i64 this, i64 argsArray) -> i64
        imports.import("env", "func_apply", EntityType::Function(16));
        // Import index 80: func_bind — Type 12 (uses shadow stack for bound args)
        imports.import("env", "func_bind", EntityType::Function(12));
        // Import index 100: object_rest: (i64, i64) -> (i64)
        imports.import("env", "object_rest", EntityType::Function(2));
        // Import index 101: obj_spread: (i64, i64) -> ()
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
        imports.import(
            "env",
            "reflect_prevent_extensions",
            EntityType::Function(31),
        );
        // Import index 163: reflect_get_own_property_descriptor: (i64 target, i64 prop) -> i64
        imports.import(
            "env",
            "reflect_get_own_property_descriptor",
            EntityType::Function(28),
        );
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
        imports.import(
            "env",
            "number_proto_to_exponential",
            EntityType::Function(2),
        );
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
        imports.import(
            "env",
            "reference_error_constructor",
            EntityType::Function(3),
        );
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
        imports.import(
            "env",
            "arraybuffer_proto_byte_length",
            EntityType::Function(3),
        );
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
        imports.import(
            "env",
            "dataview_proto_set_float64",
            EntityType::Function(16),
        );
        imports.import(
            "env",
            "dataview_proto_set_float32",
            EntityType::Function(16),
        );
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
        imports.import(
            "env",
            "uint8clampedarray_constructor",
            EntityType::Function(16),
        );
        imports.import("env", "int16array_constructor", EntityType::Function(16));
        imports.import("env", "uint16array_constructor", EntityType::Function(16));
        imports.import("env", "int32array_constructor", EntityType::Function(16));
        imports.import("env", "uint32array_constructor", EntityType::Function(16));
        imports.import("env", "float32array_constructor", EntityType::Function(16));
        imports.import("env", "float64array_constructor", EntityType::Function(16));
        imports.import("env", "bigint64array_constructor", EntityType::Function(16));
        imports.import(
            "env",
            "biguint64array_constructor",
            EntityType::Function(16),
        );
        // ── TypedArray prototype imports ──
        // Import index 306: typedarray_proto_length: (i64) -> i64
        imports.import("env", "typedarray_proto_length", EntityType::Function(3));
        // Import index 307: typedarray_proto_byte_length: (i64) -> i64
        imports.import(
            "env",
            "typedarray_proto_byte_length",
            EntityType::Function(3),
        );
        // Import index 308: typedarray_proto_byte_offset: (i64) -> i64
        imports.import(
            "env",
            "typedarray_proto_byte_offset",
            EntityType::Function(3),
        );
        // Import index 309: typedarray_proto_set: (i64, i64, i64) -> i64
        imports.import("env", "typedarray_proto_set", EntityType::Function(16));
        // Import index 310: typedarray_proto_slice: (i64, i64, i64) -> i64
        imports.import("env", "typedarray_proto_slice", EntityType::Function(16));
        // Import index 311: typedarray_proto_subarray: (i64, i64, i64) -> i64
        imports.import("env", "typedarray_proto_subarray", EntityType::Function(16));
        // Import index 312: create_global_object: () -> i64
        imports.import("env", "create_global_object", EntityType::Function(4));
        // Import index 313: create_exception: (i64) -> i64
        imports.import("env", "create_exception", EntityType::Function(3));
        // Import index 314: exception_value: (i64) -> i64
        imports.import("env", "exception_value", EntityType::Function(3));
        // Import index 315: private_get: (i64, i32) -> i64
        imports.import("env", "private_get", EntityType::Function(8));
        // Import index 316: private_set: (i64, i32, i64) -> i64
        imports.import("env", "private_set", EntityType::Function(32));
        // Import index 317: private_has: (i64, i32) -> i64
        imports.import("env", "private_has", EntityType::Function(8));
        // Import index 320: proxy_trap_get: (i64 proxy, i32 name_id) -> i64
        imports.import("env", "proxy_trap_get", EntityType::Function(8));
        // Import index 321: proxy_trap_set: (i64 proxy, i32 name_id, i64 value) -> ()
        imports.import("env", "proxy_trap_set", EntityType::Function(9));
        // Import index 322: proxy_trap_delete: (i64 proxy, i32 name_id) -> i64
        imports.import("env", "proxy_trap_delete", EntityType::Function(8));
        // Import index 323: get_builtin_global: (i64) -> i64
        imports.import("env", "get_builtin_global", EntityType::Function(3));
        // Import index 324: new_target: (i64) -> i64
        imports.import("env", "new_target", EntityType::Function(3));
        // Import index 325: new_target_set: (i64) -> i64
        imports.import("env", "new_target_set", EntityType::Function(3));
        // Import index 326: create_unmapped_arguments_object: (i64, i64) -> i64
        imports.import(
            "env",
            "create_unmapped_arguments_object",
            EntityType::Function(2),
        );
        // Import index 327: create_mapped_arguments_object: (i64, i64, i64) -> i64
        imports.import(
            "env",
            "create_mapped_arguments_object",
            EntityType::Function(16),
        );
        // Import index 328: typedarray_proto_fill: (i64, i64, i64, i64) -> i64 (receiver, value, start, end)
        imports.import("env", "typedarray_proto_fill", EntityType::Function(17));
        // Import index 329: typedarray_proto_reverse: (i64) -> i64
        imports.import("env", "typedarray_proto_reverse", EntityType::Function(3));
        // Import index 330: typedarray_proto_index_of: (i64, i64, i64) -> i64
        imports.import("env", "typedarray_proto_index_of", EntityType::Function(16));
        // Import index 331: typedarray_proto_last_index_of: (i64, i64, i64) -> i64
        imports.import(
            "env",
            "typedarray_proto_last_index_of",
            EntityType::Function(16),
        );
        // Import index 332: typedarray_proto_includes: (i64, i64, i64) -> i64
        imports.import("env", "typedarray_proto_includes", EntityType::Function(16));
        // Import index 333: typedarray_proto_join: (i64, i64) -> i64
        imports.import("env", "typedarray_proto_join", EntityType::Function(2));
        // Import index 334: typedarray_proto_to_string: (i64) -> i64
        imports.import("env", "typedarray_proto_to_string", EntityType::Function(3));
        // Import index 335: typedarray_proto_copy_within: (i64, i64, i64, i64) -> i64 (receiver, target, start, end)
        imports.import(
            "env",
            "typedarray_proto_copy_within",
            EntityType::Function(17),
        );
        // Import index 336: typedarray_proto_at: (i64, i64) -> i64
        imports.import("env", "typedarray_proto_at", EntityType::Function(2));
        // ── TypedArray 新增原型方法: 回调方法 (Type 12 shadow stack) ──
        // Import index 337: typedarray_proto_for_each: Type 12
        imports.import("env", "typedarray_proto_for_each", EntityType::Function(12));
        // Import index 338: typedarray_proto_map: Type 12
        imports.import("env", "typedarray_proto_map", EntityType::Function(12));
        // Import index 339: typedarray_proto_filter: Type 12
        imports.import("env", "typedarray_proto_filter", EntityType::Function(12));
        // Import index 340: typedarray_proto_reduce: Type 12
        imports.import("env", "typedarray_proto_reduce", EntityType::Function(12));
        // Import index 341: typedarray_proto_reduce_right: Type 12
        imports.import(
            "env",
            "typedarray_proto_reduce_right",
            EntityType::Function(12),
        );
        // Import index 342: typedarray_proto_find: Type 12
        imports.import("env", "typedarray_proto_find", EntityType::Function(12));
        // Import index 343: typedarray_proto_find_index: Type 12
        imports.import(
            "env",
            "typedarray_proto_find_index",
            EntityType::Function(12),
        );
        // Import index 344: typedarray_proto_some: Type 12
        imports.import("env", "typedarray_proto_some", EntityType::Function(12));
        // Import index 345: typedarray_proto_every: Type 12
        imports.import("env", "typedarray_proto_every", EntityType::Function(12));
        // Import index 346: typedarray_proto_sort: Type 12
        imports.import("env", "typedarray_proto_sort", EntityType::Function(12));
        // ── TypedArray 迭代器方法: (i64) -> i64 ──
        // Import index 347: typedarray_proto_entries: (i64) -> i64
        imports.import("env", "typedarray_proto_entries", EntityType::Function(3));
        // Import index 348: typedarray_proto_keys: (i64) -> i64
        imports.import("env", "typedarray_proto_keys", EntityType::Function(3));
        // Import index 349: typedarray_proto_values: (i64) -> i64
        imports.import("env", "typedarray_proto_values", EntityType::Function(3));
        // ── ScopeRecord eval bridge ──
        // Import index 350: scope_record_create: (i64) -> i64
        imports.import("env", "scope_record_create", EntityType::Function(3));
        // Import index 351: scope_record_add_binding: (i64, i64, i64, i64, i64) -> ()
        imports.import("env", "scope_record_add_binding", EntityType::Function(34));
        // Import index 352: eval_get_binding: (i64, i64) -> i64
        imports.import("env", "eval_get_binding", EntityType::Function(2));
        // Import index 353: eval_set_binding: (i64, i64, i64) -> i64
        imports.import("env", "eval_set_binding", EntityType::Function(16));
        // Import index 354: eval_has_binding: (i64, i64) -> i64
        imports.import("env", "eval_has_binding", EntityType::Function(2));
        // Import index 355: eval_super_base: (i64) -> i64
        imports.import("env", "eval_super_base", EntityType::Function(3));
        // Import index 356: scope_record_set_meta: (i64, i64, i64) -> i64
        imports.import("env", "scope_record_set_meta", EntityType::Function(16));
        // Import index 357: scope_record_destroy: (i64) -> ()
        imports.import("env", "scope_record_destroy", EntityType::Function(0));
        // ── WeakRef imports ──
        // Import index 358: weakref_constructor: Type 12 (shadow stack)
        imports.import("env", "weakref_constructor", EntityType::Function(12));
        // Import index 359: weakref_proto_deref: (i64) -> i64
        imports.import("env", "weakref_proto_deref", EntityType::Function(3));
        // ── FinalizationRegistry imports ──
        // Import index 360: finalization_registry_constructor: Type 12 (shadow stack)
        imports.import(
            "env",
            "finalization_registry_constructor",
            EntityType::Function(12),
        );
        // Import index 361: finalization_registry_proto_register: Type 12 (shadow stack)
        imports.import(
            "env",
            "finalization_registry_proto_register",
            EntityType::Function(12),
        );
        // Import index 362: finalization_registry_proto_unregister: (i64, i64) -> i64
        imports.import(
            "env",
            "finalization_registry_proto_unregister",
            EntityType::Function(2),
        );
        // ── SharedArrayBuffer imports ──
        // Import index 363: sharedarraybuffer_constructor: (i64) -> i64
        imports.import(
            "env",
            "sharedarraybuffer_constructor",
            EntityType::Function(3),
        );
        // Import index 364: sharedarraybuffer_proto_byte_length: (i64) -> i64
        imports.import(
            "env",
            "sharedarraybuffer_proto_byte_length",
            EntityType::Function(3),
        );
        // Import index 365: sharedarraybuffer_proto_slice: (i64, i64, i64) -> i64
        imports.import(
            "env",
            "sharedarraybuffer_proto_slice",
            EntityType::Function(16),
        );
        // Import index 366: sharedarraybuffer_proto_species: (i64) -> i64
        imports.import(
            "env",
            "sharedarraybuffer_proto_species",
            EntityType::Function(3),
        );
        // ── Atomics imports ──
        // Import index 367: atomics_load: (i64, i64) -> i64
        imports.import("env", "atomics_load", EntityType::Function(2));
        // Import index 368: atomics_store: (i64, i64, i64) -> i64
        imports.import("env", "atomics_store", EntityType::Function(16));
        // Import index 369: atomics_add: (i64, i64, i64) -> i64
        imports.import("env", "atomics_add", EntityType::Function(16));
        // Import index 370: atomics_sub: (i64, i64, i64) -> i64
        imports.import("env", "atomics_sub", EntityType::Function(16));
        // Import index 371: atomics_and: (i64, i64, i64) -> i64
        imports.import("env", "atomics_and", EntityType::Function(16));
        // Import index 372: atomics_or: (i64, i64, i64) -> i64
        imports.import("env", "atomics_or", EntityType::Function(16));
        // Import index 373: atomics_xor: (i64, i64, i64) -> i64
        imports.import("env", "atomics_xor", EntityType::Function(16));
        // Import index 374: atomics_exchange: (i64, i64, i64) -> i64
        imports.import("env", "atomics_exchange", EntityType::Function(16));
        // Import index 375: atomics_compare_exchange: (i64, i64, i64, i64) -> i64
        imports.import("env", "atomics_compare_exchange", EntityType::Function(17));
        // Import index 376: atomics_is_lock_free: (i64) -> i64
        imports.import("env", "atomics_is_lock_free", EntityType::Function(3));
        // Import index 377: atomics_wait: (i64, i64, i64, i64) -> i64
        imports.import("env", "atomics_wait", EntityType::Function(17));
        // Import index 378: atomics_notify: (i64, i64, i64) -> i64
        imports.import("env", "atomics_notify", EntityType::Function(16));
        // Import index 379: atomics_wait_async: (i64, i64, i64, i64) -> i64
        imports.import("env", "atomics_wait_async", EntityType::Function(17));
        // Import index 380: async_iterator_from: (i64) -> i64
        imports.import("env", "async_iterator_from", EntityType::Function(3));
        // Import index 381: object.group_by: (i64, i64) -> i64
        imports.import("env", "object.group_by", EntityType::Function(2));
        // Import index 382: map.group_by: (i64, i64) -> i64
        imports.import("env", "map.group_by", EntityType::Function(2));
        // Import index 383: symbol_property_key: (i64) -> i32
        imports.import("env", "symbol_property_key", EntityType::Function(10));
        // Import index 384: array.from — Type 12 (shadow stack variadic)
        imports.import("env", "array.from", EntityType::Function(12));
        // Import index 385: obj_get_by_index: (i64, i32) -> i64 — Type 8
        imports.import("env", "obj_get_by_index", EntityType::Function(8));
        // Import index 386: typedarray_set_by_index: (i64, i32, i64) -> i64 — Type 32
        imports.import("env", "typedarray_set_by_index", EntityType::Function(32));
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
        // 从 HOST_IMPORT_NAMES 自动生成 builtin → WASM 函数索引的映射
        {
            let name_to_idx: std::collections::HashMap<&str, u32> = HOST_IMPORT_NAMES
                .iter()
                .enumerate()
                .map(|(i, name)| (*name, i as u32))
                .collect();
            for &builtin in wjsm_ir::builtin::ALL_BUILTINS {
                let name = builtin.import_name();
                if let Some(&idx) = name_to_idx.get(name) {
                    builtin_func_indices.insert(builtin, idx);
                } else {
                    // Debugger 是免调用的 no-op，安全跳过
                    if matches!(builtin, Builtin::Debugger) {
                        continue;
                    }
                    panic!(
                        "Builtin::{:?} import_name()=\"{name}\" not found in HOST_IMPORT_NAMES",
                        builtin
                    );
                }
            }
        }
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

        // Count function imports only (memories/globals share separate index spaces)
        let actual_import_count = HOST_IMPORT_NAMES.len() as u32;
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
            eval_var_base_local_idx: 0,
            gc_collect_func_idx: 22,
            obj_get_by_index_func_idx: 385,
            typedarray_set_by_index_func_idx: 386,
            alloc_counter_global_idx: 0,
            object_heap_start_global_idx: 6,
            num_ir_functions_global_idx: 7,
            shadow_stack_end_global_idx: 8,
            closure_create_func_idx: 35,
            closure_get_func_idx: 36,
            closure_get_env_idx: 37,
            native_call_func_idx: 141,
            new_target_set_func_idx: 325,
            array_proto_handle_global_idx: 0,
            arr_proto_table_base: 0,
            obj_spread_func_idx: 0,
            proxy_trap_get_func_idx: 320,
            proxy_trap_set_func_idx: 321,
            proxy_trap_delete_func_idx: 322,
            get_proto_from_ctor_func_idx: 0,
            string_eq_func_idx: 0,
            function_id_to_wasm_idx: HashMap::new(),
            object_proto_handle_global_idx: 0,
            symbol_key_func_idx: 383,
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
