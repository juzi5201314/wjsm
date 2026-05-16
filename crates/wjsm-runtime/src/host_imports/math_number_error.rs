{
    let math_abs_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.abs())
        },
    );
    let math_acos_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.acos())
        },
    );
    let math_acosh_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.acosh())
        },
    );
    let math_asin_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.asin())
        },
    );
    let math_asinh_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.asinh())
        },
    );
    let math_atan_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.atan())
        },
    );
    let math_atanh_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.atanh())
        },
    );
    let math_atan2_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let y = value_to_number(a);
            let x = value_to_number(b);
            value::encode_f64(y.atan2(x))
        },
    );
    let math_cbrt_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.cbrt())
        },
    );
    let math_ceil_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.ceil())
        },
    );
    let math_clz32_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            let n = x as i32 as u32;
            value::encode_f64(if n == 0 { 32.0 } else { n.leading_zeros() as f64 })
        },
    );
    let math_cos_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.cos())
        },
    );
    let math_cosh_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.cosh())
        },
    );
    let math_exp_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.exp())
        },
    );
    let math_expm1_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.exp_m1())
        },
    );
    let math_floor_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.floor())
        },
    );
    let math_fround_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64((x as f32) as f64)
        },
    );
    let math_hypot_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| -> i64 {
            if args_count == 0 {
                return value::encode_f64(0.0);
            }
            let mut sum = 0.0_f64;
            for i in 0..args_count as u32 {
                let val = read_shadow_arg(&mut caller, args_base, i);
                let x = value_to_number(val);
                if x.is_infinite() {
                    return value::encode_f64(f64::INFINITY);
                }
                sum += x * x;
            }
            value::encode_f64(sum.sqrt())
        },
    );
    let math_imul_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let ai = value_to_number(a) as i32;
            let bi = value_to_number(b) as i32;
            let result = (ai as i64) * (bi as i64);
            value::encode_f64((result as i32) as f64)
        },
    );
    let math_log_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.ln())
        },
    );
    let math_log1p_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.ln_1p())
        },
    );
    let math_log10_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.log10())
        },
    );
    let math_log2_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.log2())
        },
    );
    let math_max_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| -> i64 {
            if args_count == 0 {
                return value::encode_f64(f64::NEG_INFINITY);
            }
            let mut result = f64::NEG_INFINITY;
            for i in 0..args_count as u32 {
                let val = read_shadow_arg(&mut caller, args_base, i);
                let x = f64::from_bits(val as u64);
                if x > result || (x == 0.0 && result == 0.0 && x.is_sign_positive()) {
                    result = x;
                }
            }
            value::encode_f64(result)
        },
    );
    let math_min_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| -> i64 {
            if args_count == 0 {
                return value::encode_f64(f64::INFINITY);
            }
            let mut result = f64::INFINITY;
            for i in 0..args_count as u32 {
                let val = read_shadow_arg(&mut caller, args_base, i);
                let x = f64::from_bits(val as u64);
                if x < result || (x == 0.0 && result == 0.0 && x.is_sign_negative()) {
                    result = x;
                }
            }
            value::encode_f64(result)
        },
    );
    let math_pow_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let base = value_to_number(a);
            let exp = value_to_number(b);
            value::encode_f64(base.powf(exp))
        },
    );
    let math_random_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>| -> i64 {
            let mut rng = rand::thread_rng();
            value::encode_f64(rng.gen_range(0.0_f64..1.0))
        },
    );
    let math_round_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.round())
        },
    );
    let math_sign_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            if x.is_nan() {
                return value::encode_f64(f64::NAN);
            }
            if x == 0.0 {
                return value::encode_f64(if x.is_sign_positive() { 0.0 } else { -0.0 });
            }
            value::encode_f64(if x > 0.0 { 1.0 } else { -1.0 })
        },
    );
    let math_sin_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.sin())
        },
    );
    let math_sinh_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.sinh())
        },
    );
    let math_sqrt_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.sqrt())
        },
    );
    let math_tan_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.tan())
        },
    );
    let math_tanh_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.tanh())
        },
    );
    let math_trunc_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.trunc())
        },
    );
    // ── Number builtins ─────────────────────────────────────────────────────
    let number_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            if value::is_f64(arg) {
                arg
            } else if value::is_undefined(arg) || value::is_null(arg) {
                value::encode_f64(0.0)
            } else if value::is_bool(arg) {
                value::encode_f64(if value::decode_bool(arg) { 1.0 } else { 0.0 })
            } else if value::is_string(arg) {
                let s = read_value_string_bytes(&mut caller, arg).unwrap_or_default();
                let s_str = String::from_utf8_lossy(&s).to_string();
                value::encode_f64(s_str.trim().parse::<f64>().unwrap_or(f64::NAN))
            } else {
                value::encode_f64(0.0)
            }
        },
    );
    let number_is_nan_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            if value::is_f64(arg) {
                value::encode_bool(f64::from_bits(arg as u64).is_nan())
            } else if value::is_undefined(arg) || value::is_null(arg) || value::is_bool(arg)
                || value::is_string(arg) || value::is_object(arg) || value::is_function(arg)
                || value::is_closure(arg) || value::is_bound(arg) || value::is_bigint(arg)
                || value::is_symbol(arg) || value::is_regexp(arg) || value::is_array(arg)
                || value::is_iterator(arg) || value::is_enumerator(arg) || value::is_proxy(arg)
            {
                value::encode_bool(false)
            } else {
                value::encode_bool(true)
            }
        },
    );
    let number_is_finite_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            if value::is_f64(arg) {
                value::encode_bool(f64::from_bits(arg as u64).is_finite())
            } else {
                value::encode_bool(false)
            }
        },
    );
    let number_is_integer_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            if value::is_f64(arg) {
                let x = value_to_number(arg);
                value::encode_bool(x.is_finite() && x == x.trunc())
            } else {
                value::encode_bool(false)
            }
        },
    );
    let number_is_safe_integer_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            if value::is_f64(arg) {
                let x = value_to_number(arg);
                let is_int = x.is_finite() && x == x.trunc();
                value::encode_bool(is_int && x.abs() <= 9007199254740991.0)
            } else {
                value::encode_bool(false)
            }
        },
    );
    let number_parse_int_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64, radix_val: i64| -> i64 {
            let input_str = if value::is_string(arg) {
                let s = read_value_string_bytes(&mut caller, arg).unwrap_or_default();
                String::from_utf8_lossy(&s).to_string()
            } else if value::is_f64(arg) {
                let x = value_to_number(arg);
                if x.is_nan() { return value::encode_f64(f64::NAN); }
                if x.is_infinite() { return value::encode_f64(f64::NAN); }
                format_number_js(x)
            } else if value::is_bool(arg) {
                if value::decode_bool(arg) { "1" } else { "0" }.to_string()
            } else {
                return value::encode_f64(f64::NAN);
            };
            let trimmed = input_str.trim();
            if trimmed.is_empty() {
                return value::encode_f64(f64::NAN);
            }
            let radix = if value::is_undefined(radix_val) {
                0
            } else if value::is_f64(radix_val) {
                let r = f64::from_bits(radix_val as u64);
                if r.is_nan() || r.is_infinite() {
                    return value::encode_f64(f64::NAN);
                }
                r as i32
            } else {
                0
            };
            if radix != 0 && (radix < 2 || radix > 36) {
                return value::encode_f64(f64::NAN);
            }
            let (actual_radix, parse_str): (i32, &str) = if radix == 0 {
                if trimmed.starts_with("0x") || trimmed.starts_with("0X") {
                    (16, &trimmed[2..])
                } else {
                    (10, &trimmed[..])
                }
            } else {
                let s: &str = if (radix == 16) && (trimmed.starts_with("0x") || trimmed.starts_with("0X")) {
                    &trimmed[2..]
                } else {
                    &trimmed[..]
                };
                (radix, s)
            };
            if parse_str.is_empty() {
                return value::encode_f64(f64::NAN);
            }
            let valid_chars: String = parse_str
                .chars()
                .take_while(|c| {
                    let digit = if c.is_ascii_digit() {
                        *c as u32 - '0' as u32
                    } else if c.is_ascii_alphabetic() {
                        c.to_ascii_lowercase() as u32 - 'a' as u32 + 10
                    } else {
                        return false;
                    };
                    digit < actual_radix as u32
                })
                .collect();
            if valid_chars.is_empty() {
                return value::encode_f64(f64::NAN);
            }
            match i64::from_str_radix(&valid_chars, actual_radix as u32) {
                Ok(v) => value::encode_f64(v as f64),
                Err(_) => value::encode_f64(f64::NAN),
            }
        },
    );
    let number_parse_float_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            if !value::is_string(arg) {
                if value::is_f64(arg) {
                    return arg;
                }
                return value::encode_f64(f64::NAN);
            }
            let s = read_value_string_bytes(&mut caller, arg).unwrap_or_default();
            let s_str = String::from_utf8_lossy(&s).to_string();
            let trimmed = s_str.trim();
            if trimmed.is_empty() {
                return value::encode_f64(f64::NAN);
            }
            let mut end = 0;
            let bytes = trimmed.as_bytes();
            if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
                end += 1;
            }
            let _digit_start = end;
            let mut has_digit = false;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
                has_digit = true;
            }
            if end < bytes.len() && bytes[end] == b'.' {
                end += 1;
                while end < bytes.len() && bytes[end].is_ascii_digit() {
                    end += 1;
                    has_digit = true;
                }
            }
            if !has_digit {
                return value::encode_f64(f64::NAN);
            }
            if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
                end += 1;
                if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
                    end += 1;
                }
                let exp_start = end;
                while end < bytes.len() && bytes[end].is_ascii_digit() {
                    end += 1;
                }
                if end == exp_start {
                    end -= if end > 0 && (bytes[end - 1] == b'+' || bytes[end - 1] == b'-') { 1 } else { 0 };
                    if end > 0 && (bytes[end - 1] == b'e' || bytes[end - 1] == b'E') {
                        end -= 1;
                    }
                }
            }
            if end == 0 {
                return value::encode_f64(f64::NAN);
            }
            let float_str = &trimmed[..end];
            match float_str.parse::<f64>() {
                Ok(v) => value::encode_f64(v),
                Err(_) => value::encode_f64(f64::NAN),
            }
        },
    );
    let number_proto_to_string_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, this_val: i64, radix_val: i64| -> i64 {
            if !value::is_f64(this_val) {
                return store_runtime_string(&caller, "NaN".to_string());
            }
            let x = f64::from_bits(this_val as u64);
            let radix = if value::is_undefined(radix_val) || value::is_null(radix_val) {
                10
            } else if value::is_f64(radix_val) {
                let r = f64::from_bits(radix_val as u64) as i32;
                if r < 2 || r > 36 {
                    return store_runtime_string(&caller, "NaN".to_string());
                }
                r
            } else {
                10
            };
            if x.is_nan() {
                return store_runtime_string(&caller, "NaN".to_string());
            }
            if x.is_infinite() {
                return store_runtime_string(&caller, if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string());
            }
            if radix == 10 {
                let s = format_number_js(x);
                return store_runtime_string(&caller, s);
            }
            let int_part = x.trunc() as i64;
            let result = format_radix(int_part, radix as u32);
            store_runtime_string(&caller, result)
        },
    );
    let number_proto_value_of_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            if value::is_f64(this_val) {
                this_val
            } else {
                value::encode_f64(0.0)
            }
        },
    );
    let number_proto_to_fixed_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, this_val: i64, digits_val: i64| -> i64 {
            if !value::is_f64(this_val) {
                return store_runtime_string(&caller, "NaN".to_string());
            }
            let x = f64::from_bits(this_val as u64);
            let digits = if value::is_undefined(digits_val) || value::is_null(digits_val) {
                0
            } else if value::is_f64(digits_val) {
                f64::from_bits(digits_val as u64) as i32
            } else {
                0
            };
            if digits < 0 || digits > 100 {
                return store_runtime_string(&caller, "RangeError: toFixed() digits argument must be between 0 and 100".to_string());
            }
            if x.is_nan() {
                return store_runtime_string(&caller, "NaN".to_string());
            }
            if x.is_infinite() {
                return store_runtime_string(&caller, if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string());
            }
            let s = format!("{:.1$}", x, digits as usize);
            store_runtime_string(&caller, s)
        },
    );
    let number_proto_to_exponential_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, this_val: i64, digits_val: i64| -> i64 {
            if !value::is_f64(this_val) {
                return store_runtime_string(&caller, "NaN".to_string());
            }
            let x = f64::from_bits(this_val as u64);
            if x.is_nan() {
                return store_runtime_string(&caller, "NaN".to_string());
            }
            if x.is_infinite() {
                return store_runtime_string(&caller, if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string());
            }
            let digits = if value::is_undefined(digits_val) || value::is_null(digits_val) {
                -1i32
            } else if value::is_f64(digits_val) {
                f64::from_bits(digits_val as u64) as i32
            } else {
                -1
            };
            if x == 0.0 {
                if digits > 0 {
                    let s = format!("0.{}e+0", "0".repeat(digits as usize));
                    return store_runtime_string(&caller, s);
                }
                return store_runtime_string(&caller, "0e+0".to_string());
            }
            let s = if digits >= 0 {
                format!("{:.1$e}", x, digits as usize)
            } else {
                format!("{:e}", x)
            };
            let s = normalize_exponent(&s);
            store_runtime_string(&caller, s)
        },
    );
    let number_proto_to_precision_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, this_val: i64, digits_val: i64| -> i64 {
            if !value::is_f64(this_val) {
                return store_runtime_string(&caller, "NaN".to_string());
            }
            let x = f64::from_bits(this_val as u64);
            if x.is_nan() {
                return store_runtime_string(&caller, "NaN".to_string());
            }
            if x.is_infinite() {
                return store_runtime_string(&caller, if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string());
            }
            let precision = if value::is_undefined(digits_val) || value::is_null(digits_val) {
                -1i32
            } else if value::is_f64(digits_val) {
                f64::from_bits(digits_val as u64) as i32
            } else {
                -1
            };
            if precision < 1 || precision > 21 {
                if value::is_undefined(digits_val) {
                    let s = format_number_js(x);
                    return store_runtime_string(&caller, s);
                }
                return store_runtime_string(&caller, "RangeError: toPrecision() argument must be between 1 and 21".to_string());
            }
            let s = format!("{:.1$}", x, precision as usize);
            store_runtime_string(&caller, s)
        },
    );
    // ── Boolean builtins ────────────────────────────────────────────────────
    let boolean_constructor_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            value::encode_bool(value::is_truthy(arg))
        },
    );
    let boolean_proto_to_string_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            if value::is_bool(this_val) {
                store_runtime_string(&caller, if value::decode_bool(this_val) { "true" } else { "false" }.to_string())
            } else {
                store_runtime_string(&caller, "false".to_string())
            }
        },
    );
    let boolean_proto_value_of_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            if value::is_bool(this_val) {
                this_val
            } else {
                value::encode_bool(false)
            }
        },
    );
    // ── Error builtins ────────────────────────────────────────────────────
    let error_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            create_error_object(&mut caller, "Error", arg)
        },
    );
    let type_error_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            create_error_object(&mut caller, "TypeError", arg)
        },
    );
    let range_error_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            create_error_object(&mut caller, "RangeError", arg)
        },
    );
    let syntax_error_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            create_error_object(&mut caller, "SyntaxError", arg)
        },
    );
    let reference_error_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            create_error_object(&mut caller, "ReferenceError", arg)
        },
    );
    let uri_error_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            create_error_object(&mut caller, "URIError", arg)
        },
    );
    let eval_error_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            create_error_object(&mut caller, "EvalError", arg)
        },
    );
    let error_proto_to_string_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            if !value::is_object(this_val) {
                return store_runtime_string(&caller, "Error".to_string());
            }
            let obj_ptr = resolve_handle_idx(
                &mut caller,
                value::decode_object_handle(this_val) as usize,
            );
            let name = obj_ptr
                .and_then(|p| read_object_property_by_name(&mut caller, p, "name"))
                .and_then(|v| read_value_string_bytes(&mut caller, v))
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_else(|| "Error".to_string());
            let obj_ptr2 = resolve_handle_idx(
                &mut caller,
                value::decode_object_handle(this_val) as usize,
            );
            let message = obj_ptr2
                .and_then(|p| read_object_property_by_name(&mut caller, p, "message"))
                .and_then(|v| read_value_string_bytes(&mut caller, v))
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            if message.is_empty() {
                store_runtime_string(&caller, name)
            } else {
                store_runtime_string(&caller, format!("{}: {}", name, message))
            }
        },
    );

    // ── Map / Set helper: SameValueZero equality ──────────────────────
    vec![
        math_abs_fn.into(),    // 193
        math_acos_fn.into(),   // 194
        math_acosh_fn.into(),  // 195
        math_asin_fn.into(),   // 196
        math_asinh_fn.into(),  // 197
        math_atan_fn.into(),   // 198
        math_atanh_fn.into(),  // 199
        math_atan2_fn.into(),  // 200
        math_cbrt_fn.into(),   // 201
        math_ceil_fn.into(),   // 202
        math_clz32_fn.into(),  // 203
        math_cos_fn.into(),    // 204
        math_cosh_fn.into(),   // 205
        math_exp_fn.into(),    // 206
        math_expm1_fn.into(),  // 207
        math_floor_fn.into(),  // 208
        math_fround_fn.into(), // 209
        math_hypot_fn.into(),  // 210
        math_imul_fn.into(),   // 211
        math_log_fn.into(),    // 212
        math_log1p_fn.into(),  // 213
        math_log10_fn.into(),  // 214
        math_log2_fn.into(),   // 215
        math_max_fn.into(),    // 216
        math_min_fn.into(),    // 217
        math_pow_fn.into(),    // 218
        math_random_fn.into(), // 219
        math_round_fn.into(),  // 220
        math_sign_fn.into(),   // 221
        math_sin_fn.into(),    // 222
        math_sinh_fn.into(),   // 223
        math_sqrt_fn.into(),   // 224
        math_tan_fn.into(),    // 225
        math_tanh_fn.into(),   // 226
        math_trunc_fn.into(),  // 227
        // ── Number imports ──
        number_constructor_fn.into(),       // 228
        number_is_nan_fn.into(),            // 229
        number_is_finite_fn.into(),         // 230
        number_is_integer_fn.into(),        // 231
        number_is_safe_integer_fn.into(),   // 232
        number_parse_int_fn.into(),         // 233
        number_parse_float_fn.into(),       // 234
        number_proto_to_string_fn.into(),   // 235
        number_proto_value_of_fn.into(),    // 236
        number_proto_to_fixed_fn.into(),    // 237
        number_proto_to_exponential_fn.into(), // 238
        number_proto_to_precision_fn.into(),  // 239
        // ── Boolean imports ──
        boolean_constructor_fn.into(),      // 240
        boolean_proto_to_string_fn.into(),  // 241
        boolean_proto_value_of_fn.into(),   // 242
        // ── Error imports ──
        error_constructor_fn.into(),           // 243
        type_error_constructor_fn.into(),      // 244
        range_error_constructor_fn.into(),     // 245
        syntax_error_constructor_fn.into(),    // 246
        reference_error_constructor_fn.into(), // 247
        uri_error_constructor_fn.into(),       // 248
        eval_error_constructor_fn.into(),      // 249
        error_proto_to_string_fn.into(),       // 250
    ]
}
