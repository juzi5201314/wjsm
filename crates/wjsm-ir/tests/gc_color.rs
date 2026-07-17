use wjsm_ir::value::{
    GC_COLOR_MASK, TAG_ARRAY, TAG_ENUMERATOR, TAG_ITERATOR, encode_array_hole,
    encode_bigint_handle, encode_bool, encode_bound_idx, encode_closure_idx, encode_exception,
    encode_f64, encode_function_idx, encode_handle, encode_native_callable_idx, encode_null,
    encode_object_handle, encode_proxy_handle, encode_regexp_handle, encode_runtime_string_handle,
    encode_scope_record_handle, encode_string_ptr, encode_symbol_handle, encode_typeof_undefined,
    encode_undefined, is_handle_backed_reference, strip_gc_color,
};

#[test]
fn all_handle_backed_values_preserve_identity_after_color_stripping() {
    assert_eq!(GC_COLOR_MASK, 0x0000_0FC0_0000_0000);
    let values = [
        encode_object_handle(1),
        encode_handle(TAG_ARRAY, 2),
        encode_function_idx(3),
        encode_closure_idx(4),
        encode_bound_idx(5),
        encode_native_callable_idx(6),
        encode_bigint_handle(7),
        encode_symbol_handle(8),
        encode_regexp_handle(9),
        encode_proxy_handle(10),
        encode_scope_record_handle(11),
        encode_runtime_string_handle(12),
        encode_exception(13),
        encode_handle(TAG_ITERATOR, 14),
        encode_handle(TAG_ENUMERATOR, 15),
    ];

    for value in values {
        let colored = (value as u64 | GC_COLOR_MASK) as i64;
        assert!(is_handle_backed_reference(value));
        assert!(is_handle_backed_reference(colored));
        assert_eq!(strip_gc_color(colored), value);
    }
}

#[test]
fn scalar_values_never_encode_gc_color_bits() {
    let scalars = [
        encode_f64(42.5),
        encode_string_ptr(123),
        encode_typeof_undefined(),
        encode_array_hole(),
        encode_bool(false),
        encode_bool(true),
        encode_null(),
        encode_undefined(),
    ];

    for value in scalars {
        assert_eq!(value as u64 & GC_COLOR_MASK, 0);
        assert!(!is_handle_backed_reference(value));
    }
}

#[test]
fn stripping_color_preserves_raw_f64_payload_bits() {
    let raw_f64 = encode_f64(f64::from_bits(0x3FF0_0FC0_0000_0000));
    assert_ne!(raw_f64 as u64 & GC_COLOR_MASK, 0);
    assert!(!is_handle_backed_reference(raw_f64));
    assert_eq!(strip_gc_color(raw_f64), raw_f64);

    let colored_handle = (encode_object_handle(u32::MAX) as u64 | GC_COLOR_MASK) as i64;
    assert_eq!(
        strip_gc_color(colored_handle),
        encode_object_handle(u32::MAX)
    );
    assert_eq!(
        strip_gc_color(strip_gc_color(colored_handle)),
        strip_gc_color(colored_handle)
    );
}
