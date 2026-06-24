//! 公共 type section：user wasm 与 support module 共享相同的 type index 空间。
//!
//! wasmtime 的 `call_indirect` 要求调用方与 table 中函数的 type index 一致
//! （不仅是签名一致）。因此 support module 的 type section 必须与 user wasm
//! 完全相同，使 `call_indirect type 12` 在两个 module 中指向同一个 type。

use wasm_encoder::{TypeSection, ValType};

/// `build_shared_type_section` 中 Type 12：`(i64, i64, i32, i32) -> i64`，用于 call_indirect / native_call。
pub const JS_FUNC_TYPE_INDEX: u32 = 12;


/// 生成与 user wasm compiler_core.rs::new_with_data_base 完全一致的 type section。
/// support module 和 user wasm 都调用此函数，确保 type index 一致。
pub fn build_shared_type_section() -> TypeSection {
    let mut types = TypeSection::new();
    // Type 0: (i64) -> ()  — console_log
    types.ty().function(vec![ValType::I64], vec![]);
    // Type 1: () -> ()  — main / gc_maybe_collect
    types.ty().function(vec![], vec![]);
    // Type 2: (i64, i64) -> (i64)  — f64_mod, f64_pow
    types
        .ty()
        .function(vec![ValType::I64, ValType::I64], vec![ValType::I64]);
    // Type 3: (i64) -> (i64)  — iterator/enumerator helpers / get_proto_from_ctor
    types.ty().function(vec![ValType::I64], vec![ValType::I64]);
    // Type 4: () -> (i64)  — main return / bootstrap / init_function_props
    types.ty().function(vec![], vec![ValType::I64]);
    // Type 5: (i64, i64) -> () — unused
    types
        .ty()
        .function(vec![ValType::I64, ValType::I64], vec![]);
    // Type 6: (i64, i32, i32) -> (i64)  — JS function signature (shadow stack)
    types.ty().function(
        vec![ValType::I64, ValType::I32, ValType::I32],
        vec![ValType::I64],
    );
    // Type 7: (i32) -> (i32)  — obj_new, arr_new
    types.ty().function(vec![ValType::I32], vec![ValType::I32]);
    // Type 8: (i64, i32) -> (i64)  — obj_get, obj_delete, elem_get,
    //   proxy_trap_get/delete, native_callable_get_property, primitive_number_get_method
    types
        .ty()
        .function(vec![ValType::I64, ValType::I32], vec![ValType::I64]);
    // Type 9: (i64, i32, i64) -> ()  — obj_set, elem_set, proxy_trap_set
    types
        .ty()
        .function(vec![ValType::I64, ValType::I32, ValType::I64], vec![]);
    // Type 10: (i64) -> (i32)  — to_int32
    types.ty().function(vec![ValType::I64], vec![ValType::I32]);
    // Type 11: (i64, i64) -> (i64)  — string_concat
    types
        .ty()
        .function(vec![ValType::I64, ValType::I64], vec![ValType::I64]);
    // Type 12: (i64, i64, i32, i32) -> (i64) — JS 函数签名（含 env_obj）/ native_call / call_indirect
    types.ty().function(
        vec![ValType::I64, ValType::I64, ValType::I32, ValType::I32],
        vec![ValType::I64],
    );
    // Type 13: (i32, i64) -> (i64) — closure_create
    types
        .ty()
        .function(vec![ValType::I32, ValType::I64], vec![ValType::I64]);
    // Type 14: (i32) -> (i32) — closure_get_func
    types.ty().function(vec![ValType::I32], vec![ValType::I32]);
    // Type 15: (i32) -> (i64) — closure_get_env
    types.ty().function(vec![ValType::I32], vec![ValType::I64]);
    // Type 16: (i64, i64, i64) -> (i64) — 3-arg array functions
    types.ty().function(
        vec![ValType::I64, ValType::I64, ValType::I64],
        vec![ValType::I64],
    );
    // Type 17: (i64, i64, i64, i64) -> (i64) — 4-arg array functions
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
    // Type 20: (i32, i32, i32, i32) -> (i64) — regex_create
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
    // Type 26: (i32, i32) -> (i32) — string_eq
    types
        .ty()
        .function(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
    // Type 27: (i64, i64, i64) -> (i64)  — jsx_create_element
    types.ty().function(
        vec![ValType::I64, ValType::I64, ValType::I64],
        vec![ValType::I64],
    );
    // Type 28: (i64, i64) -> (i64)  — proxy_create, etc.
    types
        .ty()
        .function(vec![ValType::I64, ValType::I64], vec![ValType::I64]);
    // Type 29: (i64, i64, i64) -> (i64)  — reflect_get, etc.
    types.ty().function(
        vec![ValType::I64, ValType::I64, ValType::I64],
        vec![ValType::I64],
    );
    // Type 30: (i64, i64, i64, i64) -> (i64)  — reflect_set, etc.
    types.ty().function(
        vec![ValType::I64, ValType::I64, ValType::I64, ValType::I64],
        vec![ValType::I64],
    );
    // Type 31: (i64) -> (i64)  — reflect_is_extensible, etc.
    types.ty().function(vec![ValType::I64], vec![ValType::I64]);
    // Type 32: (i64, i32, i64) -> (i64) — private_set
    types.ty().function(
        vec![ValType::I64, ValType::I32, ValType::I64],
        vec![ValType::I64],
    );
    // Type 33: (i32, i32) -> () — console varargs
    types
        .ty()
        .function(vec![ValType::I32, ValType::I32], vec![]);
    // Type 34: (i64, i64, i64, i64, i64) -> (i64) — scope_record_add_binding
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
    // Type 35: (i32, i32, i32) -> (i32) — gc_alloc_slow
    types.ty().function(
        vec![ValType::I32, ValType::I32, ValType::I32],
        vec![ValType::I32],
    );
    // Type 36: () -> (i32) — gc_take_freed_handle
    types.ty().function(vec![], vec![ValType::I32]);
    types
}
