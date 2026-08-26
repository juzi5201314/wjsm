use wjsm_ir::value::{
    GC_COLOR_MASK, GcColorMask, TAG_ARRAY, TAG_ENUMERATOR, TAG_ITERATOR, apply_gc_color,
    encode_array_hole, encode_bigint_handle, encode_bool, encode_bound_idx, encode_closure_idx,
    encode_exception, encode_f64, encode_function_idx, encode_handle, encode_native_callable_idx,
    encode_null, encode_object_handle, encode_proxy_handle, encode_regexp_handle,
    encode_runtime_string_handle, encode_scope_record_handle, encode_string_ptr,
    encode_symbol_handle, encode_typeof_undefined, encode_undefined, is_f64,
    is_handle_backed_reference, is_string, strip_gc_color,
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

#[test]
fn inline_ascii_round_trips_all_lengths_and_boundaries() {
    let mut output = [0_u8; 6];
    for length in 0..=6 {
        let input = [b'a', b'\0', b'Z', b'~', b'0', 0x7f];
        let value = wjsm_ir::value::encode_inline_ascii(&input[..length]).expect("ASCII SSO");
        assert!(wjsm_ir::value::is_inline_string(value));
        assert_eq!(wjsm_ir::value::inline_string_len(value), Some(length as u8));
        assert_eq!(
            wjsm_ir::value::decode_inline_ascii(value, &mut output),
            Some(&input[..length])
        );
        assert!(!wjsm_ir::value::is_handle_backed_reference(value));
        assert_eq!(wjsm_ir::value::gc_color_bits(value), 0);
    }
}

#[test]
fn inline_ascii_rejects_non_ascii_and_seven_units() {
    assert!(wjsm_ir::value::encode_inline_ascii(b"abcdefg").is_none());
    assert!(wjsm_ir::value::encode_inline_ascii(&[0x80]).is_none());
    let empty = wjsm_ir::value::encode_inline_ascii(b"").expect("empty SSO");
    assert!(wjsm_ir::value::is_falsy(empty));
    let nonempty = wjsm_ir::value::encode_inline_ascii(b"a").expect("nonempty SSO");
    assert!(!wjsm_ir::value::is_falsy(nonempty));
}

#[test]
fn inline_ascii_is_not_a_number_or_heap_reference() {
    let mut output = [0_u8; 6];
    for length in 0..=6 {
        let input = b"abcdef".get(..length).unwrap();
        let value = wjsm_ir::value::encode_inline_ascii(input).expect("ASCII SSO");
        assert!(is_string(value));
        assert!(!is_f64(value));
        assert!(!is_handle_backed_reference(value));
        assert_eq!(strip_gc_color(value), value);
        assert_eq!(apply_gc_color(value, GcColorMask::EMPTY), value);
        assert_eq!(wjsm_ir::value::gc_color_bits(value), 0);
        assert_eq!(
            wjsm_ir::value::decode_inline_ascii(value, &mut output)
                .unwrap()
                .len(),
            length
        );
    }
}

#[test]
fn inline_ascii_rejects_reserved_bits_and_noncanonical_tail() {
    let value = wjsm_ir::value::encode_inline_ascii(b"a").expect("ASCII SSO");
    assert!(!wjsm_ir::value::is_inline_string(
        (value as u64 | (1_u64 << 42)) as i64
    ));
    assert!(!wjsm_ir::value::is_inline_string(
        (value as u64 | (1_u64 << 7)) as i64
    ));
}

#[test]
fn inline_ascii_round_trip_preserves_nul_and_del() {
    let input = [0_u8, 0x7f, b'a', 0, 0x7f, b'z'];
    let encoded = wjsm_ir::value::encode_inline_ascii(&input).expect("ASCII SSO");
    let mut output = [0_u8; 6];
    assert_eq!(
        wjsm_ir::value::decode_inline_ascii(encoded, &mut output),
        Some(input.as_slice())
    );
}

#[test]
fn inline_latin1_round_trips_all_lengths() {
    for length in 0..=wjsm_ir::value::INLINE_STRING_LATIN1_MAX_LEN {
        let input: Vec<u8> = (0..length).map(|index| 0x60 + index as u8).collect();
        let value = wjsm_ir::value::encode_inline_latin1(&input).expect("Latin-1 SSO");
        assert!(wjsm_ir::value::is_inline_latin1(value));
        assert!(wjsm_ir::value::is_inline_string(value));
        assert_eq!(wjsm_ir::value::inline_string_len(value), Some(length as u8));
        let mut output = [0_u8; wjsm_ir::value::INLINE_STRING_MAX_LEN];
        assert_eq!(
            wjsm_ir::value::decode_inline_latin1(value, &mut output),
            Some(input.as_slice())
        );
    }
}

#[test]
fn inline_latin1_is_not_a_number_or_heap_reference() {
    let value = wjsm_ir::value::encode_inline_latin1(&[0xe9]).expect("Latin-1 SSO");
    assert!(is_string(value));
    assert!(!is_f64(value));
    assert!(!is_handle_backed_reference(value));
    assert!(!wjsm_ir::value::is_inline_ascii(value));
}
