use wasmtime::*;
use wjsm_ir::value;
use chrono::{Datelike, DateTime, TimeZone, Timelike, Utc};
use num_bigint;
use num_traits::cast::ToPrimitive;
use rand::Rng;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use crate::types::*;
use crate::runtime::*;

pub(crate) fn create_host_functions(store: &mut Store<RuntimeState>) -> Vec<(usize, Func)> {
    macro_rules! dataview_get_fn {
        ($name:ident, $size:expr, $conv:expr) => {
            let $name = Func::wrap(
                &mut *store,
                |mut caller: Caller<'_, RuntimeState>, this_val: i64, byte_offset: i64| -> i64 {
                    let offset = value::decode_f64(byte_offset) as u32;
                    let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
                    let dv_handle = match obj_ptr {
                        Some(ptr) => {
                            match read_object_property_by_name(&mut caller, ptr, "__dataview_handle__") {
                                Some(v) => value::decode_f64(v) as usize,
                                None => return value::encode_undefined(),
                            }
                        }
                        None => return value::encode_undefined(),
                    };
                    let (buf_handle, dv_offset, dv_length) = {
                        let dv_table = caller.data().dataview_table.lock().expect("dataview_table mutex");
                        if dv_handle < dv_table.len() {
                            let e = &dv_table[dv_handle];
                            (e.buffer_handle, e.byte_offset, e.byte_length)
                        } else { return value::encode_undefined(); }
                    };
                    let abs_offset = dv_offset as usize + offset as usize;
                    let ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
                    if let Some(buf_entry) = ab_table.get(buf_handle as usize) {
                        if offset + $size as u32 > dv_length || abs_offset + $size > buf_entry.data.len() {
                            *caller.data().runtime_error.lock().expect("error mutex") =
                                Some("RangeError: Offset is outside the bounds of the DataView".to_string());
                            return value::encode_undefined();
                        }
                        let bytes = &buf_entry.data[abs_offset..abs_offset + $size];
                        return $conv(bytes);
                    }
                    value::encode_undefined()
                },
            );
        };
    }

    macro_rules! dataview_set_fn {
        ($name:ident, $size:expr, $write:expr) => {
            let $name = Func::wrap(
                &mut *store,
                |mut caller: Caller<'_, RuntimeState>, this_val: i64, byte_offset: i64, value_arg: i64| -> i64 {
                    let offset = value::decode_f64(byte_offset) as u32;
                    let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
                    let dv_handle = match obj_ptr {
                        Some(ptr) => {
                            match read_object_property_by_name(&mut caller, ptr, "__dataview_handle__") {
                                Some(v) => value::decode_f64(v) as usize,
                                None => return value::encode_undefined(),
                            }
                        }
                        None => return value::encode_undefined(),
                    };
                    let (buf_handle, dv_offset, dv_length) = {
                        let dv_table = caller.data().dataview_table.lock().expect("dataview_table mutex");
                        if dv_handle < dv_table.len() {
                            let e = &dv_table[dv_handle];
                            (e.buffer_handle, e.byte_offset, e.byte_length)
                        } else { return value::encode_undefined(); }
                    };
                    let abs_offset = dv_offset as usize + offset as usize;
                    let mut ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
                    if let Some(buf_entry) = ab_table.get_mut(buf_handle as usize) {
                        if offset + $size as u32 > dv_length || abs_offset + $size > buf_entry.data.len() {
                            *caller.data().runtime_error.lock().expect("error mutex") =
                                Some("RangeError: Offset is outside the bounds of the DataView".to_string());
                            return value::encode_undefined();
                        }
                        let bytes = $write(value_arg);
                        buf_entry.data[abs_offset..abs_offset + $size].copy_from_slice(&bytes[..$size]);
                    }
                    value::encode_undefined()
                },
            );
        };
    }

    macro_rules! typedarray_constructor {
        ($name:ident, $size:expr) => {
            let $name = Func::wrap(
                &mut *store,
                |mut caller: Caller<'_, RuntimeState>, buffer: i64, byte_offset: i64, length: i64| -> i64 {
                    let offset = value::decode_f64(byte_offset) as u32;
                    let len = value::decode_f64(length) as u32;
                    let buf_handle = {
                        let obj_ptr = resolve_handle(&mut caller, buffer);
                        match obj_ptr {
                            Some(ptr) => {
                                let h = read_object_property_by_name(&mut caller, ptr, "__arraybuffer_handle__");
                                match h { Some(v) => value::decode_f64(v) as u32, None => return value::encode_undefined() }
                            }
                            None => return value::encode_undefined(),
                        }
                    };
                    let handle;
                    {
                        let mut table = caller.data().typedarray_table.lock().expect("typedarray_table mutex");
                        handle = table.len() as u32;
                        table.push(TypedArrayEntry { buffer_handle: buf_handle, byte_offset: offset, length: len, element_size: $size });
                    }
                    let obj = alloc_host_object_from_caller(&mut caller, 4);
                    let handle_val = value::encode_f64(handle as f64);
                    let _ = define_host_data_property_from_caller(&mut caller, obj, "__typedarray_handle__", handle_val);
                    let len_val = value::encode_f64(len as f64);
                    let _ = define_host_data_property_from_caller(&mut caller, obj, "length", len_val);
                    let bl_val = value::encode_f64((len * $size as u32) as f64);
                    let _ = define_host_data_property_from_caller(&mut caller, obj, "byteLength", bl_val);
                    let bo_val = value::encode_f64(offset as f64);
                    let _ = define_host_data_property_from_caller(&mut caller, obj, "byteOffset", bo_val);
                    obj
                },
            );
        };
    }

    let throw_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            let rendered = render_value(&mut caller, val).unwrap_or_else(|_| "unknown".to_string());
            let mut buffer = caller
                .data()
                .output
                .lock()
                .expect("runtime output buffer mutex should not be poisoned");
            writeln!(&mut *buffer, "Uncaught exception: {rendered}").ok();
            *caller
                .data()
                .runtime_error
                .lock()
                .expect("runtime error mutex") = Some(format!("Uncaught exception: {rendered}"));
        },
    );

    // ── Import 4: iterator_from(i64) → i64 ──────────────────────────────

    let fetch_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, url_val: i64| -> i64 {
            let url_str = if value::is_string(url_val) {
                if value::is_runtime_string_handle(url_val) {
                    let handle = value::decode_runtime_string_handle(url_val) as usize;
                    caller
                        .data()
                        .runtime_strings
                        .lock()
                        .expect("runtime strings mutex")
                        .get(handle)
                        .cloned()
                        .unwrap_or_default()
                } else {
                    read_string(&mut caller, value::decode_string_ptr(url_val)).unwrap_or_default()
                }
            } else {
                String::new()
            };

            if url_str.starts_with("data:") {
                // Handle data: URLs inline (no network)
                let body = url_str.split(',').nth(1).unwrap_or("").to_string();
                let decoded = urlencoding_decode(&body);
                store_runtime_string(&caller, decoded)
            } else {
                // Network fetch — use ureq if available
                let body = format!("[fetch blocked: {url_str}]");
                store_runtime_string(&caller, body)
            }
        },
    );

    // ── Import 32: json_stringify(i64) → i64 ──────────────────────────────

    let json_stringify_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            let json_str = runtime_json_stringify(&mut caller, val);
            store_runtime_string(&caller, json_str)
        },
    );

    // ── Import 33: json_parse(i64) → i64 ──────────────────────────────────

    let json_parse_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            let json_str = if value::is_string(val) {
                if value::is_runtime_string_handle(val) {
                    let handle = value::decode_runtime_string_handle(val) as usize;
                    caller
                        .data()
                        .runtime_strings
                        .lock()
                        .expect("runtime strings mutex")
                        .get(handle)
                        .cloned()
                        .unwrap_or_default()
                } else {
                    read_string(&mut caller, value::decode_string_ptr(val)).unwrap_or_default()
                }
            } else {
                String::new()
            };
            // For now, just return the string as-is (simplified parse)
            store_runtime_string(&caller, json_str)
        },
    );
    // ── Import 34: closure_create(i32, i64) -> i64 ────────────────────────────

    let closure_create_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, func_idx: i32, env_obj: i64| -> i64 {
            let mut closures = caller.data().closures.lock().expect("closures mutex");
            let idx = closures.len() as u32;
            closures.push(ClosureEntry {
                func_idx: func_idx as u32,
                env_obj,
            });
            value::encode_closure_idx(idx)
        },
    );
    // ── Import 35: closure_get_func(i32) -> i32 ─────────────────────────────

    let closure_get_func_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, closure_idx: i32| -> i32 {
            let closures = caller.data().closures.lock().expect("closures mutex");
            closures
                .get(closure_idx as usize)
                .map(|e| e.func_idx as i32)
                .unwrap_or(-1)
        },
    );
    // ── Import 36: closure_get_env(i32) -> i64 ─────────────────────────────

    let closure_get_env_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, closure_idx: i32| -> i64 {
            let closures = caller.data().closures.lock().expect("closures mutex");
            closures
                .get(closure_idx as usize)
                .map(|e| e.env_obj)
                .unwrap_or_else(value::encode_undefined)
        },
    );
    // ── Array method host functions (imports 37-48) ────────────────────

    let abort_shadow_stack_overflow_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, shadow_sp: i32, args_bytes: i32, stack_end: i32| {
            let mut buffer = caller
                .data()
                .output
                .lock()
                .expect("runtime output buffer mutex should not be poisoned");
            writeln!(
                &mut *buffer,
                "shadow stack overflow: sp=0x{shadow_sp:x} + {args_bytes} bytes > end=0x{stack_end:x}"
            ).ok();
            *caller
                .data()
                .runtime_error
                .lock()
                .expect("runtime error mutex") = Some(format!(
                "shadow stack overflow: sp={shadow_sp} + {args_bytes} > end={stack_end}"
            ));
        },
    );

    // ── func_call (#78): Function.prototype.call ────────────────────────────
    // 签名: (i64 func, i64 this_val, i64 args_base, i32 args_count) -> i64

    let func_call_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>,
         func: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 { resolve_and_call(&mut caller, func, this_val, args_base, args_count) },
    );

    // ── func_apply (#79): Function.prototype.apply ──────────────────────────

    let func_apply_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, func: i64, this_val: i64, args_array: i64| -> i64 {
            func_apply_impl(&mut caller, func, this_val, args_array)
        },
    );

    // ── func_bind (#80): Function.prototype.bind ────────────────────────────

    let func_bind_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>,
         func: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 { func_bind_impl(&mut caller, func, this_val, args_base, args_count) },
    );

    // ── object_rest (#81): Exclude specified keys from object ───────────────

    let native_call_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>,
         callable: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let args = (0..args_count.max(0))
                .map(|index| read_shadow_arg(&mut caller, args_base, index as u32))
                .collect();
            call_native_callable_with_args_from_caller(&mut caller, callable, this_val, args)
                .unwrap_or_else(value::encode_undefined)
        },
    );

    // ── Import 146: register_module_namespace(i64, i64) -> () ──────────────
    // 将模块命名空间对象注册到运行时缓存

    let eval_direct_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, code: i64, scope_env: i64| -> i64 {
            perform_eval_from_caller(&mut caller, code, Some(scope_env))
        },
    );

    let eval_indirect_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, code: i64| -> i64 {
            perform_eval_from_caller(&mut caller, code, None)
        },
    );

    let jsx_create_element_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, tag: i64, props: i64, children: i64| -> i64 {
            let obj = alloc_host_object_from_caller(&mut caller, 4);
            let _ = define_host_data_property_from_caller(
                &mut caller, obj, "type", tag,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller, obj, "props", props,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller, obj, "children", children,
            );
            obj
        },
    );

    // ── Proxy / Reflect ────────────────────────────────────────────────────────

    fn value_to_number(arg: i64) -> f64 {
        if value::is_f64(arg) {
            value::decode_f64(arg)
        } else if value::is_bool(arg) {
            if value::decode_bool(arg) { 1.0 } else { 0.0 }
        } else if value::is_undefined(arg) {
            f64::NAN
        } else if value::is_null(arg) {
            0.0
        } else {
            f64::NAN
        }
    }

    let math_abs_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.abs())
        },
    );

    let math_acos_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.acos())
        },
    );

    let math_acosh_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.acosh())
        },
    );

    let math_asin_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.asin())
        },
    );

    let math_asinh_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.asinh())
        },
    );

    let math_atan_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.atan())
        },
    );

    let math_atanh_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.atanh())
        },
    );

    let math_atan2_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let y = value_to_number(a);
            let x = value_to_number(b);
            value::encode_f64(y.atan2(x))
        },
    );

    let math_cbrt_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.cbrt())
        },
    );

    let math_ceil_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.ceil())
        },
    );

    let math_clz32_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            let n = x as i32 as u32;
            value::encode_f64(if n == 0 { 32.0 } else { n.leading_zeros() as f64 })
        },
    );

    let math_cos_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.cos())
        },
    );

    let math_cosh_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.cosh())
        },
    );

    let math_exp_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.exp())
        },
    );

    let math_expm1_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.exp_m1())
        },
    );

    let math_floor_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.floor())
        },
    );

    let math_fround_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64((x as f32) as f64)
        },
    );

    let math_hypot_fn = Func::wrap(
        &mut *store,
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
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let ai = value_to_number(a) as i32;
            let bi = value_to_number(b) as i32;
            let result = (ai as i64) * (bi as i64);
            value::encode_f64((result as i32) as f64)
        },
    );

    let math_log_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.ln())
        },
    );

    let math_log1p_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.ln_1p())
        },
    );

    let math_log10_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.log10())
        },
    );

    let math_log2_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.log2())
        },
    );

    let math_max_fn = Func::wrap(
        &mut *store,
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
        &mut *store,
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
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let base = value_to_number(a);
            let exp = value_to_number(b);
            value::encode_f64(base.powf(exp))
        },
    );

    let math_random_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>| -> i64 {
            let mut rng = rand::thread_rng();
            value::encode_f64(rng.gen_range(0.0_f64..1.0))
        },
    );

    let math_round_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.round())
        },
    );

    let math_sign_fn = Func::wrap(
        &mut *store,
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
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.sin())
        },
    );

    let math_sinh_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.sinh())
        },
    );

    let math_sqrt_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.sqrt())
        },
    );

    let math_tan_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.tan())
        },
    );

    let math_tanh_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.tanh())
        },
    );

    let math_trunc_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let x = value_to_number(arg);
            value::encode_f64(x.trunc())
        },
    );
    // ── Number builtins ─────────────────────────────────────────────────────

    let number_constructor_fn = Func::wrap(
        &mut *store,
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
        &mut *store,
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
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            if value::is_f64(arg) {
                value::encode_bool(f64::from_bits(arg as u64).is_finite())
            } else {
                value::encode_bool(false)
            }
        },
    );

    let number_is_integer_fn = Func::wrap(
        &mut *store,
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
        &mut *store,
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
        &mut *store,
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
        &mut *store,
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
        &mut *store,
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
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            if value::is_f64(this_val) {
                this_val
            } else {
                value::encode_f64(0.0)
            }
        },
    );

    let number_proto_to_fixed_fn = Func::wrap(
        &mut *store,
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
        &mut *store,
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
        &mut *store,
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
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            value::encode_bool(value::is_truthy(arg))
        },
    );

    let boolean_proto_to_string_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            if value::is_bool(this_val) {
                store_runtime_string(&caller, if value::decode_bool(this_val) { "true" } else { "false" }.to_string())
            } else {
                store_runtime_string(&caller, "false".to_string())
            }
        },
    );

    let boolean_proto_value_of_fn = Func::wrap(
        &mut *store,
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
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            create_error_object(&mut caller, "Error", arg)
        },
    );

    let type_error_constructor_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            create_error_object(&mut caller, "TypeError", arg)
        },
    );

    let range_error_constructor_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            create_error_object(&mut caller, "RangeError", arg)
        },
    );

    let syntax_error_constructor_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            create_error_object(&mut caller, "SyntaxError", arg)
        },
    );

    let reference_error_constructor_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            create_error_object(&mut caller, "ReferenceError", arg)
        },
    );

    let uri_error_constructor_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            create_error_object(&mut caller, "URIError", arg)
        },
    );

    let eval_error_constructor_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            create_error_object(&mut caller, "EvalError", arg)
        },
    );

    let error_proto_to_string_fn = Func::wrap(
        &mut *store,
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

    let map_constructor_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, _arg: i64| -> i64 {
            let handle;
            {
                let mut table = caller.data().map_table.lock().expect("map table mutex");
                table.push(MapEntry { keys: Vec::new(), values: Vec::new() });
                handle = table.len() as u32 - 1;
            }
            let (set_fn, get_fn, has_fn, delete_fn, clear_fn, size_fn, for_each_fn, keys_fn, values_fn, entries_fn) = {
                let state = caller.data();
                (
                    create_map_set_method(state, MapSetMethodKind::MapSet),
                    create_map_set_method(state, MapSetMethodKind::MapGet),
                    create_map_set_method(state, MapSetMethodKind::Has),
                    create_map_set_method(state, MapSetMethodKind::Delete),
                    create_map_set_method(state, MapSetMethodKind::Clear),
                    create_map_set_method(state, MapSetMethodKind::Size),
                    create_map_set_method(state, MapSetMethodKind::ForEach),
                    create_map_set_method(state, MapSetMethodKind::Keys),
                    create_map_set_method(state, MapSetMethodKind::Values),
                    create_map_set_method(state, MapSetMethodKind::Entries),
                )
            };
            let obj = alloc_host_object_from_caller(&mut caller, 11);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__map_handle__", handle_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "set", set_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "get", get_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "has", has_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "delete", delete_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "clear", clear_fn);
            // TODO: size should be a getter accessor property per ES spec, but current
            // architecture does not support call_indirect for host import functions.
            // Tracked as a known compliance gap — currently exposed as a method: m.size()
            let _ = define_host_data_property_from_caller(&mut caller, obj, "size", size_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "forEach", for_each_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "keys", keys_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "values", values_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "entries", entries_fn);
            obj
        },
    );

    let map_proto_set_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64, val: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_undefined();
            }
            let obj_ptr = resolve_handle_idx(
                &mut caller,
                value::decode_object_handle(this_val) as usize,
            );
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__map_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let mut table = caller.data().map_table.lock().expect("map table mutex");
            if handle >= table.len() {
                return value::encode_undefined();
            }
            let entry = &mut table[handle];
            for i in 0..entry.keys.len() {
                if same_value_zero(entry.keys[i], key) {
                    entry.values[i] = val;
                    return this_val;
                }
            }
            entry.keys.push(key);
            entry.values.push(val);
            this_val
        },
    );

    let map_proto_get_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_undefined();
            }
            let obj_ptr = resolve_handle_idx(
                &mut caller,
                value::decode_object_handle(this_val) as usize,
            );
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__map_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let table = caller.data().map_table.lock().expect("map table mutex");
            if handle >= table.len() {
                return value::encode_undefined();
            }
            let entry = &table[handle];
            for i in 0..entry.keys.len() {
                if same_value_zero(entry.keys[i], key) {
                    return entry.values[i];
                }
            }
            value::encode_undefined()
        },
    );

    // ── Set host functions ────────────────────────────────────────────

    let set_constructor_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, _arg: i64| -> i64 {
            let handle;
            {
                let mut table = caller.data().set_table.lock().expect("set table mutex");
                table.push(SetEntry { values: Vec::new() });
                handle = table.len() as u32 - 1;
            }
            let (add_fn, has_fn, delete_fn, clear_fn, size_fn, for_each_fn, keys_fn, values_fn, entries_fn) = {
                let state = caller.data();
                (
                    create_map_set_method(state, MapSetMethodKind::SetAdd),
                    create_map_set_method(state, MapSetMethodKind::Has),
                    create_map_set_method(state, MapSetMethodKind::Delete),
                    create_map_set_method(state, MapSetMethodKind::Clear),
                    create_map_set_method(state, MapSetMethodKind::Size),
                    create_map_set_method(state, MapSetMethodKind::ForEach),
                    create_map_set_method(state, MapSetMethodKind::Keys),
                    create_map_set_method(state, MapSetMethodKind::Values),
                    create_map_set_method(state, MapSetMethodKind::Entries),
                )
            };
            let obj = alloc_host_object_from_caller(&mut caller, 10);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__set_handle__", handle_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "add", add_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "has", has_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "delete", delete_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "clear", clear_fn);
            // TODO: ES spec requires `size` to be a getter accessor property, but the current
            // WASM architecture doesn't support calling NativeCallable via call_indirect.
            // Using a data property (method) as a workaround.
            let _ = define_host_data_property_from_caller(&mut caller, obj, "size", size_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "forEach", for_each_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "keys", keys_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "values", values_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "entries", entries_fn);
            obj
        },
    );

    let set_proto_add_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, val: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_undefined();
            }
            let obj_ptr = resolve_handle_idx(
                &mut caller,
                value::decode_object_handle(this_val) as usize,
            );
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__set_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let mut table = caller.data().set_table.lock().expect("set table mutex");
            if handle >= table.len() {
                return value::encode_undefined();
            }
            let entry = &mut table[handle];
            for i in 0..entry.values.len() {
                if same_value_zero(entry.values[i], val) {
                    return this_val;
                }
            }
            entry.values.push(val);
            this_val
        },
    );

    // ── Map/Set shared host functions (dispatch at runtime) ──────────

    let map_set_has_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_bool(false);
            }
            let obj_ptr = resolve_handle_idx(
                &mut caller,
                value::decode_object_handle(this_val) as usize,
            );
            if let Some(op) = obj_ptr {
                let map_handle = read_object_property_by_name(&mut caller, op, "__map_handle__");
                if let Some(mh) = map_handle {
                    let handle = value::decode_f64(mh) as usize;
                    let table = caller.data().map_table.lock().expect("map table mutex");
                    if handle < table.len() {
                        let entry = &table[handle];
                        for i in 0..entry.keys.len() {
                            if same_value_zero(entry.keys[i], key) {
                                return value::encode_bool(true);
                            }
                        }
                    }
                    return value::encode_bool(false);
                }
                let set_handle = read_object_property_by_name(&mut caller, op, "__set_handle__");
                if let Some(sh) = set_handle {
                    let handle = value::decode_f64(sh) as usize;
                    let table = caller.data().set_table.lock().expect("set table mutex");
                    if handle < table.len() {
                        let entry = &table[handle];
                        for i in 0..entry.values.len() {
                            if same_value_zero(entry.values[i], key) {
                                return value::encode_bool(true);
                            }
                        }
                    }
                    return value::encode_bool(false);
                }
            }
            value::encode_bool(false)
        },
    );

    let map_set_delete_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_bool(false);
            }
            let obj_ptr = resolve_handle_idx(
                &mut caller,
                value::decode_object_handle(this_val) as usize,
            );
            if let Some(op) = obj_ptr {
                let map_handle = read_object_property_by_name(&mut caller, op, "__map_handle__");
                if let Some(mh) = map_handle {
                    let handle = value::decode_f64(mh) as usize;
                    let mut table = caller.data().map_table.lock().expect("map table mutex");
                    if handle < table.len() {
                        let entry = &mut table[handle];
                        for i in 0..entry.keys.len() {
                            if same_value_zero(entry.keys[i], key) {
                                entry.keys.remove(i);
                                entry.values.remove(i);
                                return value::encode_bool(true);
                            }
                        }
                    }
                    return value::encode_bool(false);
                }
                let set_handle = read_object_property_by_name(&mut caller, op, "__set_handle__");
                if let Some(sh) = set_handle {
                    let handle = value::decode_f64(sh) as usize;
                    let mut table = caller.data().set_table.lock().expect("set table mutex");
                    if handle < table.len() {
                        let entry = &mut table[handle];
                        for i in 0..entry.values.len() {
                            if same_value_zero(entry.values[i], key) {
                                entry.values.remove(i);
                                return value::encode_bool(true);
                            }
                        }
                    }
                    return value::encode_bool(false);
                }
            }
            value::encode_bool(false)
        },
    );

    let map_set_clear_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_undefined();
            }
            let obj_ptr = resolve_handle_idx(
                &mut caller,
                value::decode_object_handle(this_val) as usize,
            );
            if let Some(op) = obj_ptr {
                let map_handle = read_object_property_by_name(&mut caller, op, "__map_handle__");
                if let Some(mh) = map_handle {
                    let handle = value::decode_f64(mh) as usize;
                    let mut table = caller.data().map_table.lock().expect("map table mutex");
                    if handle < table.len() {
                        table[handle].keys.clear();
                        table[handle].values.clear();
                    }
                    return value::encode_undefined();
                }
                let set_handle = read_object_property_by_name(&mut caller, op, "__set_handle__");
                if let Some(sh) = set_handle {
                    let handle = value::decode_f64(sh) as usize;
                    let mut table = caller.data().set_table.lock().expect("set table mutex");
                    if handle < table.len() {
                        table[handle].values.clear();
                    }
                    return value::encode_undefined();
                }
            }
            value::encode_undefined()
        },
    );

    let map_set_get_size_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            if !value::is_object(this_val) {
                return value::encode_f64(0.0);
            }
            let obj_ptr = resolve_handle_idx(
                &mut caller,
                value::decode_object_handle(this_val) as usize,
            );
            if let Some(op) = obj_ptr {
                let map_handle = read_object_property_by_name(&mut caller, op, "__map_handle__");
                if let Some(mh) = map_handle {
                    let handle = value::decode_f64(mh) as usize;
                    let table = caller.data().map_table.lock().expect("map table mutex");
                    if handle < table.len() {
                        return value::encode_f64(table[handle].keys.len() as f64);
                    }
                    return value::encode_f64(0.0);
                }
                let set_handle = read_object_property_by_name(&mut caller, op, "__set_handle__");
                if let Some(sh) = set_handle {
                    let handle = value::decode_f64(sh) as usize;
                    let table = caller.data().set_table.lock().expect("set table mutex");
                    if handle < table.len() {
                        return value::encode_f64(table[handle].values.len() as f64);
                    }
                    return value::encode_f64(0.0);
                }
            }
            value::encode_f64(0.0)
        },
    );

    let map_set_for_each_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _this_val: i64| -> i64 {
            value::encode_undefined()
        },
    );

    let map_set_keys_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _this_val: i64| -> i64 {
            value::encode_undefined()
        },
    );

    let map_set_values_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _this_val: i64| -> i64 {
            value::encode_undefined()
        },
    );

    let map_set_entries_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _this_val: i64| -> i64 {
            value::encode_undefined()
        },
    );

    // ── WeakMap host functions ───────────────────────────────────────────

    let weakmap_constructor_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, _arg: i64| -> i64 {
            let handle;
            {
                let mut table = caller.data().weakmap_table.lock().expect("weakmap_table mutex");
                handle = table.len() as u32;
                table.push(WeakMapEntry { map: HashMap::new() });
            }
            let (set_fn, get_fn, has_fn, delete_fn) = {
                let state = caller.data();
                (
                    create_weakmap_method(state, WeakMapMethodKind::Set),
                    create_weakmap_method(state, WeakMapMethodKind::Get),
                    create_weakmap_method(state, WeakMapMethodKind::Has),
                    create_weakmap_method(state, WeakMapMethodKind::Delete),
                )
            };
            let obj = alloc_host_object_from_caller(&mut caller, 5);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__weakmap_handle__", handle_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "set", set_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "get", get_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "has", has_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "delete", delete_fn);
            obj
        },
    );

    let weakmap_proto_set_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64, val: i64| -> i64 {
            if !is_object_key(key) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Invalid value used as weak map key".to_string());
                return this_val;
            }
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__weakmap_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            {
                let mut table = caller.data().weakmap_table.lock().expect("weakmap_table mutex");
                if handle < table.len() {
                    table[handle].map.insert(key_handle, val);
                }
            }
            this_val
        },
    );

    let weakmap_proto_get_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                return value::encode_undefined();
            }
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__weakmap_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller.data().weakmap_table.lock().expect("weakmap_table mutex");
            if handle < table.len() {
                if let Some(&val) = table[handle].map.get(&key_handle) {
                    return val;
                }
            }
            value::encode_undefined()
        },
    );

    let weakmap_proto_has_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__weakmap_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller.data().weakmap_table.lock().expect("weakmap_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].map.contains_key(&key_handle));
            }
            value::encode_bool(false)
        },
    );

    let weakmap_proto_delete_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__weakmap_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let mut table = caller.data().weakmap_table.lock().expect("weakmap_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].map.remove(&key_handle).is_some());
            }
            value::encode_bool(false)
        },
    );

    // ── WeakSet host functions ───────────────────────────────────────────

    let weakset_constructor_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, _arg: i64| -> i64 {
            let handle;
            {
                let mut table = caller.data().weakset_table.lock().expect("weakset_table mutex");
                handle = table.len() as u32;
                table.push(WeakSetEntry { set: HashSet::new() });
            }
            let (add_fn, has_fn, delete_fn) = {
                let state = caller.data();
                (
                    create_weakset_method(state, WeakSetMethodKind::Add),
                    create_weakset_method(state, WeakSetMethodKind::Has),
                    create_weakset_method(state, WeakSetMethodKind::Delete),
                )
            };
            let obj = alloc_host_object_from_caller(&mut caller, 4);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__weakset_handle__", handle_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "add", add_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "has", has_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "delete", delete_fn);
            obj
        },
    );

    let weakset_proto_add_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: Invalid value used in weak set".to_string());
                return this_val;
            }
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__weakset_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            {
                let mut table = caller.data().weakset_table.lock().expect("weakset_table mutex");
                if handle < table.len() {
                    table[handle].set.insert(key_handle);
                }
            }
            this_val
        },
    );

    let weakset_proto_has_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__weakset_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let table = caller.data().weakset_table.lock().expect("weakset_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].set.contains(&key_handle));
            }
            value::encode_bool(false)
        },
    );

    let weakset_proto_delete_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, key: i64| -> i64 {
            if !is_object_key(key) {
                return value::encode_bool(false);
            }
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__weakset_handle__"));
            let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
            let key_handle = value::decode_object_handle(key);
            let mut table = caller.data().weakset_table.lock().expect("weakset_table mutex");
            if handle < table.len() {
                return value::encode_bool(table[handle].set.remove(&key_handle));
            }
            value::encode_bool(false)
        },
    );

    let arraybuffer_constructor_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, byte_length: i64| -> i64 {
            let len_f64 = value::decode_f64(byte_length);
            if len_f64 < 0.0 {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("RangeError: Invalid array buffer length".to_string());
                return value::encode_undefined();
            }
            let len = len_f64 as u32;
            let handle;
            {
                let mut table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
                handle = table.len() as u32;
                table.push(ArrayBufferEntry { data: vec![0; len as usize] });
            }
            let obj = alloc_host_object_from_caller(&mut caller, 4);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__arraybuffer_handle__", handle_val);
            let bl_val = value::encode_f64(len as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "byteLength", bl_val);
            obj
        },
    );

    let arraybuffer_proto_byte_length_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            match obj_ptr {
                Some(ptr) => {
                    match read_object_property_by_name(&mut caller, ptr, "byteLength") {
                        Some(v) => v,
                        None => value::encode_f64(0.0),
                    }
                }
                None => value::encode_f64(0.0),
            }
        },
    );

    let arraybuffer_proto_slice_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, begin: i64, end: i64| -> i64 {
            let begin_idx = value::decode_f64(begin) as u32;
            let end_idx = value::decode_f64(end) as u32;
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let (buf_handle, buf_len) = match obj_ptr {
                Some(ptr) => {
                    let h = read_object_property_by_name(&mut caller, ptr, "__arraybuffer_handle__");
                    let bl = read_object_property_by_name(&mut caller, ptr, "byteLength");
                    match (h, bl) {
                        (Some(hv), Some(lv)) => (value::decode_f64(hv) as u32, value::decode_f64(lv) as u32),
                        _ => return value::encode_undefined(),
                    }
                }
                None => return value::encode_undefined(),
            };
            let start = begin_idx.min(buf_len);
            let stop = end_idx.min(buf_len);
            let new_len = if stop > start { stop - start } else { 0 };
            let new_buf_handle;
            {
                let mut ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
                new_buf_handle = ab_table.len() as u32;
                let mut new_data = vec![0u8; new_len as usize];
                if let Some(buf_entry) = ab_table.get(buf_handle as usize) {
                    new_data.copy_from_slice(&buf_entry.data[start as usize..stop as usize]);
                }
                ab_table.push(ArrayBufferEntry { data: new_data });
            }
            let obj = alloc_host_object_from_caller(&mut caller, 4);
            let handle_val = value::encode_f64(new_buf_handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__arraybuffer_handle__", handle_val);
            let bl_val = value::encode_f64(new_len as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "byteLength", bl_val);
            obj
        },
    );

    let dataview_constructor_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, buffer: i64, byte_offset: i64, byte_length: i64| -> i64 {
            let offset = if value::is_undefined(byte_offset) { 0 } else { value::decode_f64(byte_offset) as u32 };
            let (buf_handle, buf_byte_length) = {
                let obj_ptr = resolve_handle(&mut caller, buffer);
                match obj_ptr {
                    Some(ptr) => {
                        let h = read_object_property_by_name(&mut caller, ptr, "__arraybuffer_handle__");
                        let bl = read_object_property_by_name(&mut caller, ptr, "byteLength");
                        match (h, bl) {
                            (Some(hv), Some(lv)) => (value::decode_f64(hv) as u32, value::decode_f64(lv) as u32),
                            _ => return value::encode_undefined(),
                        }
                    }
                    None => return value::encode_undefined(),
                }
            };
            let length = if value::is_undefined(byte_length) {
                buf_byte_length.saturating_sub(offset)
            } else {
                value::decode_f64(byte_length) as u32
            };
            let handle;
            {
                let mut table = caller.data().dataview_table.lock().expect("dataview_table mutex");
                handle = table.len() as u32;
                table.push(DataViewEntry { buffer_handle: buf_handle, byte_offset: offset, byte_length: length });
            }
            let obj = alloc_host_object_from_caller(&mut caller, 4);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__dataview_handle__", handle_val);
            obj
        },
    );

    dataview_get_fn!(dataview_proto_get_int8_fn, 1, |bytes: &[u8]| value::encode_f64(bytes[0] as i8 as f64));
    dataview_get_fn!(dataview_proto_get_uint8_fn, 1, |bytes: &[u8]| value::encode_f64(bytes[0] as f64));
    dataview_get_fn!(dataview_proto_get_int16_fn, 2, |bytes: &[u8]| value::encode_f64(i16::from_le_bytes([bytes[0], bytes[1]]) as f64));
    dataview_get_fn!(dataview_proto_get_uint16_fn, 2, |bytes: &[u8]| value::encode_f64(u16::from_le_bytes([bytes[0], bytes[1]]) as f64));
    dataview_get_fn!(dataview_proto_get_int32_fn, 4, |bytes: &[u8]| value::encode_f64(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64));
    dataview_get_fn!(dataview_proto_get_uint32_fn, 4, |bytes: &[u8]| value::encode_f64(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64));
    dataview_get_fn!(dataview_proto_get_float32_fn, 4, |bytes: &[u8]| value::encode_f64(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64));
    dataview_get_fn!(dataview_proto_get_float64_fn, 8, |bytes: &[u8]| f64::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]]).to_bits() as i64);

    dataview_set_fn!(dataview_proto_set_int8_fn, 1, |v: i64| (value::decode_f64(v) as i8).to_le_bytes().to_vec());
    dataview_set_fn!(dataview_proto_set_uint8_fn, 1, |v: i64| (value::decode_f64(v) as u8).to_le_bytes().to_vec());
    dataview_set_fn!(dataview_proto_set_int16_fn, 2, |v: i64| (value::decode_f64(v) as i16).to_le_bytes().to_vec());
    dataview_set_fn!(dataview_proto_set_uint16_fn, 2, |v: i64| (value::decode_f64(v) as u16).to_le_bytes().to_vec());
    dataview_set_fn!(dataview_proto_set_int32_fn, 4, |v: i64| (value::decode_f64(v) as i32).to_le_bytes().to_vec());
    dataview_set_fn!(dataview_proto_set_uint32_fn, 4, |v: i64| (value::decode_f64(v) as u32).to_le_bytes().to_vec());
    dataview_set_fn!(dataview_proto_set_float32_fn, 4, |v: i64| (value::decode_f64(v) as f32).to_le_bytes().to_vec());
    dataview_set_fn!(dataview_proto_set_float64_fn, 8, |v: i64| value::decode_f64(v).to_le_bytes().to_vec());

    let typedarray_proto_length_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            match obj_ptr {
                Some(ptr) => {
                    match read_object_property_by_name(&mut caller, ptr, "length") {
                        Some(v) => v,
                        None => value::encode_f64(0.0),
                    }
                }
                None => value::encode_f64(0.0),
            }
        },
    );

    let typedarray_proto_byte_length_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            match obj_ptr {
                Some(ptr) => {
                    match read_object_property_by_name(&mut caller, ptr, "byteLength") {
                        Some(v) => v,
                        None => value::encode_f64(0.0),
                    }
                }
                None => value::encode_f64(0.0),
            }
        },
    );

    let typedarray_proto_byte_offset_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            match obj_ptr {
                Some(ptr) => {
                    match read_object_property_by_name(&mut caller, ptr, "byteOffset") {
                        Some(v) => v,
                        None => value::encode_f64(0.0),
                    }
                }
                None => value::encode_f64(0.0),
            }
        },
    );

    let typedarray_proto_set_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _this_val: i64, _source: i64, _offset: i64| -> i64 {
            value::encode_undefined()
        },
    );

    let typedarray_proto_slice_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _this_val: i64, _begin: i64, _end: i64| -> i64 {
            value::encode_undefined()
        },
    );

    let typedarray_proto_subarray_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, _this_val: i64, _begin: i64, _end: i64| -> i64 {
            value::encode_undefined()
        },
    );

    typedarray_constructor!(int8array_constructor_fn, 1);
    typedarray_constructor!(uint8array_constructor_fn, 1);
    typedarray_constructor!(uint8clampedarray_constructor_fn, 1);
    typedarray_constructor!(int16array_constructor_fn, 2);
    typedarray_constructor!(uint16array_constructor_fn, 2);
    typedarray_constructor!(int32array_constructor_fn, 4);
    typedarray_constructor!(uint32array_constructor_fn, 4);
    typedarray_constructor!(float32array_constructor_fn, 4);
    typedarray_constructor!(float64array_constructor_fn, 8);

    let get_builtin_global_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, name_val: i64| -> i64 {
            let name = read_runtime_string(&mut caller, name_val);
            let mut native_callables = caller.data().native_callables.lock().unwrap();
            let idx = native_callables.len() as u32;
            match name.as_str() {
                "Array" => {
                    native_callables.push(NativeCallable::ArrayConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Object" => {
                    native_callables.push(NativeCallable::ObjectConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Function" => {
                    native_callables.push(NativeCallable::FunctionConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "String" => {
                    native_callables.push(NativeCallable::StringConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Boolean" => {
                    native_callables.push(NativeCallable::BooleanConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Number" => {
                    native_callables.push(NativeCallable::NumberConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Symbol" => {
                    native_callables.push(NativeCallable::SymbolConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "BigInt" => {
                    native_callables.push(NativeCallable::BigIntConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "RegExp" => {
                    native_callables.push(NativeCallable::RegExpConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Error" => {
                    native_callables.push(NativeCallable::ErrorConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "TypeError" => {
                    native_callables.push(NativeCallable::TypeErrorConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "RangeError" => {
                    native_callables.push(NativeCallable::RangeErrorConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "SyntaxError" => {
                    native_callables.push(NativeCallable::SyntaxErrorConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "ReferenceError" => {
                    native_callables.push(NativeCallable::ReferenceErrorConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "URIError" => {
                    native_callables.push(NativeCallable::URIErrorConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "EvalError" => {
                    native_callables.push(NativeCallable::EvalErrorConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "AggregateError" => {
                    native_callables.push(NativeCallable::AggregateErrorConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Map" => {
                    native_callables.push(NativeCallable::MapConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Set" => {
                    native_callables.push(NativeCallable::SetConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "WeakMap" => {
                    native_callables.push(NativeCallable::WeakMapConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "WeakSet" => {
                    native_callables.push(NativeCallable::WeakSetConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Date" => {
                    native_callables.push(NativeCallable::DateConstructorGlobal);
                    value::encode_native_callable_idx(idx)
                }
                "Promise" => {
                    native_callables.push(NativeCallable::PromiseConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "ArrayBuffer" => {
                    native_callables.push(NativeCallable::ArrayBufferConstructorGlobal);
                    value::encode_native_callable_idx(idx)
                }
                "DataView" => {
                    native_callables.push(NativeCallable::DataViewConstructorGlobal);
                    value::encode_native_callable_idx(idx)
                }
                "Int8Array" | "Uint8Array" | "Uint8ClampedArray" | "Int16Array" | "Uint16Array"
                | "Int32Array" | "Uint32Array" | "Float32Array" | "Float64Array"
                | "Float16Array" | "BigInt64Array" | "BigUint64Array" => {
                    native_callables.push(NativeCallable::TypedArrayConstructor(name.clone()));
                    value::encode_native_callable_idx(idx)
                }
                "Proxy" => {
                    native_callables.push(NativeCallable::ProxyConstructor);
                    value::encode_native_callable_idx(idx)
                }
                "Math" | "JSON" | "Reflect" | "globalThis" | "Atomics"
                | "SharedArrayBuffer" | "FinalizationRegistry" | "WeakRef"
                | "parseInt" | "parseFloat" | "isNaN" | "isFinite"
                | "decodeURI" | "decodeURIComponent" | "encodeURI" | "encodeURIComponent"
                | "Temporal" | "Intl" | "Iterator" | "AsyncIterator"
                | "$262" | "eval" | "SuppressedError" => {
                    native_callables.push(NativeCallable::StubGlobal(name.clone()));
                    value::encode_native_callable_idx(idx)
                }
                _ => value::encode_undefined(),
            }
        },
    );

    let date_constructor_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, _this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let args: Vec<i64> = if args_count > 0 {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return value::encode_undefined();
                };
                let data = memory.data(&caller);
                let base = args_base as usize;
                let mut result = Vec::with_capacity(args_count as usize);
                for i in 0..args_count as usize {
                    let offset = base + i * 8;
                    if offset + 8 <= data.len() {
                        let mut bytes = [0u8; 8];
                        bytes.copy_from_slice(&data[offset..offset + 8]);
                        result.push(i64::from_le_bytes(bytes));
                    } else {
                        result.push(value::encode_undefined());
                    }
                }
                result
            } else {
                vec![]
            };

            let ms = if args.is_empty() {
                let now = chrono::Utc::now();
                now.timestamp_millis() as f64
            } else if args.len() == 1 {
                let arg = args[0];
                if value::is_undefined(arg) {
                    let now = chrono::Utc::now();
                    now.timestamp_millis() as f64
                } else if value::is_f64(arg) {
                    let val = value::decode_f64(arg);
                    if val.is_nan() || val.is_infinite() {
                        f64::NAN
                    } else {
                        val
                    }
                } else if value::is_string(arg) {
                    let s = read_value_string_bytes(&mut caller, arg)
                        .map(|b| String::from_utf8_lossy(&b).into_owned())
                        .unwrap_or_default();
                    if s.is_empty() {
                        f64::NAN
                    } else {
                        match DateTime::parse_from_rfc3339(&s) {
                            Ok(dt) => dt.timestamp_millis() as f64,
                            Err(_) => match chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S") {
                                Ok(ndt) => ndt.and_utc().timestamp_millis() as f64,
                                Err(_) => match chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
                                    Ok(nd) => nd.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis() as f64,
                                    Err(_) => f64::NAN,
                                },
                            },
                        }
                    }
                } else {
                    f64::NAN
                }
            } else {
                date_args_to_ms(&args, false)
            };

            let state = caller.data();
            let (get_date_fn, get_day_fn, get_full_year_fn, get_hours_fn, get_milliseconds_fn,
                 get_minutes_fn, get_month_fn, get_seconds_fn, get_time_fn, get_timezone_offset_fn,
                 get_utc_date_fn, get_utc_day_fn, get_utc_full_year_fn, get_utc_hours_fn,
                 get_utc_milliseconds_fn, get_utc_minutes_fn, get_utc_month_fn, get_utc_seconds_fn,
                 set_date_fn, set_full_year_fn, set_hours_fn, set_milliseconds_fn,
                 set_minutes_fn, set_month_fn, set_seconds_fn, set_time_fn,
                 set_utc_date_fn, set_utc_full_year_fn, set_utc_hours_fn, set_utc_milliseconds_fn,
                 set_utc_minutes_fn, set_utc_month_fn, set_utc_seconds_fn,
                 to_string_fn, to_date_string_fn, to_time_string_fn, to_iso_string_fn,
                 to_utc_string_fn, to_json_fn, value_of_fn) = {
                (
                    create_date_method(state, DateMethodKind::GetDate),
                    create_date_method(state, DateMethodKind::GetDay),
                    create_date_method(state, DateMethodKind::GetFullYear),
                    create_date_method(state, DateMethodKind::GetHours),
                    create_date_method(state, DateMethodKind::GetMilliseconds),
                    create_date_method(state, DateMethodKind::GetMinutes),
                    create_date_method(state, DateMethodKind::GetMonth),
                    create_date_method(state, DateMethodKind::GetSeconds),
                    create_date_method(state, DateMethodKind::GetTime),
                    create_date_method(state, DateMethodKind::GetTimezoneOffset),
                    create_date_method(state, DateMethodKind::GetUTCDate),
                    create_date_method(state, DateMethodKind::GetUTCDay),
                    create_date_method(state, DateMethodKind::GetUTCFullYear),
                    create_date_method(state, DateMethodKind::GetUTCHours),
                    create_date_method(state, DateMethodKind::GetUTCMilliseconds),
                    create_date_method(state, DateMethodKind::GetUTCMinutes),
                    create_date_method(state, DateMethodKind::GetUTCMonth),
                    create_date_method(state, DateMethodKind::GetUTCSeconds),
                    create_date_method(state, DateMethodKind::SetDate),
                    create_date_method(state, DateMethodKind::SetFullYear),
                    create_date_method(state, DateMethodKind::SetHours),
                    create_date_method(state, DateMethodKind::SetMilliseconds),
                    create_date_method(state, DateMethodKind::SetMinutes),
                    create_date_method(state, DateMethodKind::SetMonth),
                    create_date_method(state, DateMethodKind::SetSeconds),
                    create_date_method(state, DateMethodKind::SetTime),
                    create_date_method(state, DateMethodKind::SetUTCDate),
                    create_date_method(state, DateMethodKind::SetUTCFullYear),
                    create_date_method(state, DateMethodKind::SetUTCHours),
                    create_date_method(state, DateMethodKind::SetUTCMilliseconds),
                    create_date_method(state, DateMethodKind::SetUTCMinutes),
                    create_date_method(state, DateMethodKind::SetUTCMonth),
                    create_date_method(state, DateMethodKind::SetUTCSeconds),
                    create_date_method(state, DateMethodKind::ToString),
                    create_date_method(state, DateMethodKind::ToDateString),
                    create_date_method(state, DateMethodKind::ToTimeString),
                    create_date_method(state, DateMethodKind::ToISOString),
                    create_date_method(state, DateMethodKind::ToUTCString),
                    create_date_method(state, DateMethodKind::ToJSON),
                    create_date_method(state, DateMethodKind::ValueOf),
                )
            };

            let obj = alloc_host_object_from_caller(&mut caller, 40);
            let ms_val = value::encode_f64(ms);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__date_ms__", ms_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getDate", get_date_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getDay", get_day_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getFullYear", get_full_year_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getHours", get_hours_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getMilliseconds", get_milliseconds_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getMinutes", get_minutes_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getMonth", get_month_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getSeconds", get_seconds_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getTime", get_time_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getTimezoneOffset", get_timezone_offset_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getUTCDate", get_utc_date_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getUTCDay", get_utc_day_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getUTCFullYear", get_utc_full_year_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getUTCHours", get_utc_hours_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getUTCMilliseconds", get_utc_milliseconds_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getUTCMinutes", get_utc_minutes_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getUTCMonth", get_utc_month_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "getUTCSeconds", get_utc_seconds_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setDate", set_date_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setFullYear", set_full_year_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setHours", set_hours_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setMilliseconds", set_milliseconds_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setMinutes", set_minutes_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setMonth", set_month_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setSeconds", set_seconds_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setTime", set_time_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setUTCDate", set_utc_date_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setUTCFullYear", set_utc_full_year_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setUTCHours", set_utc_hours_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setUTCMilliseconds", set_utc_milliseconds_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setUTCMinutes", set_utc_minutes_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setUTCMonth", set_utc_month_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "setUTCSeconds", set_utc_seconds_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "toString", to_string_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "toDateString", to_date_string_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "toTimeString", to_time_string_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "toISOString", to_iso_string_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "toUTCString", to_utc_string_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "toJSON", to_json_fn);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "valueOf", value_of_fn);
            obj
        },
    );

    let date_now_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>| -> i64 {
            let now = chrono::Utc::now();
            value::encode_f64(now.timestamp_millis() as f64)
        },
    );

    let date_parse_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let s = if value::is_string(arg) {
                read_value_string_bytes(&mut caller, arg)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            if s.is_empty() {
                return value::encode_f64(f64::NAN);
            }
            match DateTime::parse_from_rfc3339(&s) {
                Ok(dt) => value::encode_f64(dt.timestamp_millis() as f64),
                Err(_) => match chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S") {
                    Ok(ndt) => value::encode_f64(ndt.and_utc().timestamp_millis() as f64),
                    Err(_) => match chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S%.f") {
                        Ok(ndt) => value::encode_f64(ndt.and_utc().timestamp_millis() as f64),
                        Err(_) => match chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
                            Ok(nd) => value::encode_f64(nd.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis() as f64),
                            Err(_) => match chrono::NaiveDateTime::parse_from_str(&s, "%b %d, %Y") {
                                Ok(ndt) => value::encode_f64(ndt.and_utc().timestamp_millis() as f64),
                                Err(_) => match chrono::NaiveDateTime::parse_from_str(&s, "%B %d, %Y") {
                                    Ok(ndt) => value::encode_f64(ndt.and_utc().timestamp_millis() as f64),
                                    Err(_) => match chrono::NaiveDateTime::parse_from_str(&s, "%d %b %Y %H:%M:%S") {
                                        Ok(ndt) => value::encode_f64(ndt.and_utc().timestamp_millis() as f64),
                                        Err(_) => value::encode_f64(f64::NAN),
                                    },
                                },
                            },
                        },
                    },
                },
            }
        },
    );

    let date_utc_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let args = vec![arg];
            let ms = date_args_to_ms(&args, true);
            value::encode_f64(ms)
        },
    );

    // TODO: 当前私有字段实现仅通过 "#fieldName" 字符串作为属性键存储在对象的普通属性槽中，
    // 不符合 ECMAScript 规范的 [[PrivateElements]] 语义。任何代码都可以通过 obj["#x"] 访问，
    // 且没有基于类身份的访问控制。未来需要重构为基于类身份的私有槽机制。

    let private_get_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_name_id: i32| -> i64 {
            if !value::is_object(obj) && !value::is_function(obj) {
                *caller.data().runtime_error.lock().expect("runtime error mutex") =
                    Some("TypeError: cannot read private member from non-object".to_string());
                return value::encode_undefined();
            }
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_undefined();
            };
            match read_object_property_by_name_id(&mut caller, ptr, key_name_id as u32) {
                Some(val) => val,
                None => {
                    *caller.data().runtime_error.lock().expect("runtime error mutex") =
                        Some("TypeError: cannot read private member from an object whose class did not declare it".to_string());
                    value::encode_undefined()
                }
            }
        },
    );

    let private_set_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_name_id: i32, val: i64| -> i64 {
            if !value::is_object(obj) && !value::is_function(obj) {
                *caller.data().runtime_error.lock().expect("runtime error mutex") =
                    Some("TypeError: cannot write private member to non-object".to_string());
                return value::encode_undefined();
            }
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_undefined();
            };
            let found_slot = find_property_slot_by_name_id(&mut caller, ptr, key_name_id as u32);
            if let Some((slot_offset, _flags, _old_val)) = found_slot {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return value::encode_undefined();
                };
                let data = memory.data_mut(&mut caller);
                data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
                val
            } else {
                write_object_property_by_name_id(&mut caller, ptr, obj, key_name_id as u32, val, 0);
                val
            }
        },
    );

    let private_has_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_name_id: i32| -> i64 {
            if !value::is_object(obj) && !value::is_function(obj) {
                return value::encode_bool(false);
            }
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_bool(false);
            };
            let found = find_property_slot_by_name_id(&mut caller, ptr, key_name_id as u32);
            value::encode_bool(found.is_some())
        },
    );

    vec![
        (3, throw_fn),
        (32, fetch_fn),
        (33, json_stringify_fn),
        (34, json_parse_fn),
        (35, closure_create_fn),
        (36, closure_get_func_fn),
        (37, closure_get_env_fn),
        (77, abort_shadow_stack_overflow_fn),
        (78, func_call_fn),
        (79, func_apply_fn),
        (80, func_bind_fn),
        (141, native_call_fn),
        (148, eval_direct_fn),
        (149, eval_indirect_fn),
        (150, jsx_create_element_fn),
        (193, math_abs_fn),
        (194, math_acos_fn),
        (195, math_acosh_fn),
        (196, math_asin_fn),
        (197, math_asinh_fn),
        (198, math_atan_fn),
        (199, math_atanh_fn),
        (200, math_atan2_fn),
        (201, math_cbrt_fn),
        (202, math_ceil_fn),
        (203, math_clz32_fn),
        (204, math_cos_fn),
        (205, math_cosh_fn),
        (206, math_exp_fn),
        (207, math_expm1_fn),
        (208, math_floor_fn),
        (209, math_fround_fn),
        (210, math_hypot_fn),
        (211, math_imul_fn),
        (212, math_log_fn),
        (213, math_log1p_fn),
        (214, math_log10_fn),
        (215, math_log2_fn),
        (216, math_max_fn),
        (217, math_min_fn),
        (218, math_pow_fn),
        (219, math_random_fn),
        (220, math_round_fn),
        (221, math_sign_fn),
        (222, math_sin_fn),
        (223, math_sinh_fn),
        (224, math_sqrt_fn),
        (225, math_tan_fn),
        (226, math_tanh_fn),
        (227, math_trunc_fn),
        (228, number_constructor_fn),
        (229, number_is_nan_fn),
        (230, number_is_finite_fn),
        (231, number_is_integer_fn),
        (232, number_is_safe_integer_fn),
        (233, number_parse_int_fn),
        (234, number_parse_float_fn),
        (235, number_proto_to_string_fn),
        (236, number_proto_value_of_fn),
        (237, number_proto_to_fixed_fn),
        (238, number_proto_to_exponential_fn),
        (239, number_proto_to_precision_fn),
        (240, boolean_constructor_fn),
        (241, boolean_proto_to_string_fn),
        (242, boolean_proto_value_of_fn),
        (243, error_constructor_fn),
        (244, type_error_constructor_fn),
        (245, range_error_constructor_fn),
        (246, syntax_error_constructor_fn),
        (247, reference_error_constructor_fn),
        (248, uri_error_constructor_fn),
        (249, eval_error_constructor_fn),
        (250, error_proto_to_string_fn),
        (251, map_constructor_fn),
        (252, map_proto_set_fn),
        (253, map_proto_get_fn),
        (254, set_constructor_fn),
        (255, set_proto_add_fn),
        (256, map_set_has_fn),
        (257, map_set_delete_fn),
        (258, map_set_clear_fn),
        (259, map_set_get_size_fn),
        (260, map_set_for_each_fn),
        (261, map_set_keys_fn),
        (262, map_set_values_fn),
        (263, map_set_entries_fn),
        (264, date_constructor_fn),
        (265, date_now_fn),
        (266, date_parse_fn),
        (267, date_utc_fn),
        (268, weakmap_constructor_fn),
        (269, weakmap_proto_set_fn),
        (270, weakmap_proto_get_fn),
        (271, weakmap_proto_has_fn),
        (272, weakmap_proto_delete_fn),
        (273, weakset_constructor_fn),
        (274, weakset_proto_add_fn),
        (275, weakset_proto_has_fn),
        (276, weakset_proto_delete_fn),
        (277, arraybuffer_constructor_fn),
        (278, arraybuffer_proto_byte_length_fn),
        (279, arraybuffer_proto_slice_fn),
        (280, dataview_constructor_fn),
        (281, dataview_proto_get_float64_fn),
        (282, dataview_proto_get_float32_fn),
        (283, dataview_proto_get_int32_fn),
        (284, dataview_proto_get_uint32_fn),
        (285, dataview_proto_get_int16_fn),
        (286, dataview_proto_get_uint16_fn),
        (287, dataview_proto_get_int8_fn),
        (288, dataview_proto_get_uint8_fn),
        (289, dataview_proto_set_float64_fn),
        (290, dataview_proto_set_float32_fn),
        (291, dataview_proto_set_int32_fn),
        (292, dataview_proto_set_uint32_fn),
        (293, dataview_proto_set_int16_fn),
        (294, dataview_proto_set_uint16_fn),
        (295, dataview_proto_set_int8_fn),
        (296, dataview_proto_set_uint8_fn),
        (297, int8array_constructor_fn),
        (298, uint8array_constructor_fn),
        (299, uint8clampedarray_constructor_fn),
        (300, int16array_constructor_fn),
        (301, uint16array_constructor_fn),
        (302, int32array_constructor_fn),
        (303, uint32array_constructor_fn),
        (304, float32array_constructor_fn),
        (305, float64array_constructor_fn),
        (306, typedarray_proto_length_fn),
        (307, typedarray_proto_byte_length_fn),
        (308, typedarray_proto_byte_offset_fn),
        (309, typedarray_proto_set_fn),
        (310, typedarray_proto_slice_fn),
        (311, typedarray_proto_subarray_fn),
        (312, get_builtin_global_fn),
        (313, private_get_fn),
        (314, private_set_fn),
        (315, private_has_fn),
    ]
}
