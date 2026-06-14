use wjsm_ir::value::{
    encode_bigint_handle, encode_bool, encode_bound_idx, encode_closure_idx, encode_exception,
    encode_f64, encode_function_idx, encode_handle, encode_native_callable_idx, encode_null,
    encode_object_handle, encode_proxy_handle, encode_regexp_handle, encode_runtime_string_handle,
    encode_scope_record_handle, encode_string_ptr, encode_symbol_handle, encode_undefined,
    tag_needs_root, TAG_ARRAY, TAG_ENUMERATOR, TAG_ITERATOR,
};

#[test]
fn tag_needs_root_covers_all_handle_tags() {
    let handles: &[i64] = &[
        encode_object_handle(1),
        encode_handle(TAG_ARRAY, 2),
        encode_function_idx(3),
        encode_closure_idx(4),
        encode_bound_idx(5),
        encode_bigint_handle(6),
        encode_symbol_handle(7),
        encode_regexp_handle(8),
        encode_proxy_handle(9),
        encode_scope_record_handle(10),
        encode_native_callable_idx(11),
        encode_runtime_string_handle(12),
        encode_handle(TAG_ITERATOR, 13),
        encode_handle(TAG_ENUMERATOR, 14),
        encode_exception(15),
    ];

    for (i, val) in handles.iter().enumerate() {
        assert!(
            tag_needs_root(*val),
            "handle tag at index {i} (val={val:#018x}) should need rooting",
        );
    }
}

#[test]
fn tag_needs_root_rejects_scalars() {
    let scalars: &[i64] = &[
        encode_f64(3.14),
        encode_f64(0.0),
        encode_f64(-0.0),
        encode_undefined(),
        encode_null(),
        encode_bool(true),
        encode_bool(false),
        // Static string pointer (NOT a runtime handle): must NOT root.
        encode_string_ptr(0x1000),
    ];

    for (i, val) in scalars.iter().enumerate() {
        assert!(
            !tag_needs_root(*val),
            "scalar at index {i} (val={val:#018x}) should NOT need rooting",
        );
    }
}
