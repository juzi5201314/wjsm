use anyhow::Result;
use wasmtime::{Caller, Linker};

use crate::*;

pub(crate) fn define_timers_arrays(linker: &mut Linker<RuntimeState>, mut store: &mut Store<RuntimeState>) -> Result<()> {
    let f = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, callback: i64, delay: i64| -> i64 {
            let delay_f64 = if value::is_f64(delay) {
                f64::from_bits(delay as u64)
            } else {
                f64::NAN
            };
            let delay_ms: u64 = if delay_f64.is_nan() || delay_f64.is_sign_negative() {
                0
            } else if delay_f64 > (u32::MAX as f64) {
                u32::MAX as u64
            } else {
                delay_f64 as u64
            };
            let id = {
                let mut next_id = caller
                    .data()
                    .next_timer_id
                    .lock()
                    .expect("next_timer_id mutex");
                let id = *next_id;
                *next_id += 1;
                id
            };
            let deadline = Instant::now() + Duration::from_millis(delay_ms);
            let mut timers = caller.data().timers.lock().expect("timers mutex");
            timers.push(TimerEntry {
                id,
                deadline,
                callback,
                repeating: false,
                interval: Duration::from_millis(delay_ms),
            });
            value::encode_f64(id as f64)
        },
    );
    linker.define(&mut store, "env", "set_timeout", f)?;

    // ── Import 28: clear_timeout(i64) → () ────────────────────────────────
    let f = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, timer_id: i64| {
            if value::is_f64(timer_id) {
                let id = f64::from_bits(timer_id as u64) as u32;
                caller
                    .data()
                    .cancelled_timers
                    .lock()
                    .expect("cancelled_timers mutex")
                    .insert(id);
            }
            // For simplicity, mark as cancelled rather than removing from the vec
        },
    );
    linker.define(&mut store, "env", "clear_timeout", f)?;

    // ── Import 29: set_interval(i64, i64) → i64 ───────────────────────────
    let f = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, callback: i64, delay: i64| -> i64 {
            let delay_f64 = if value::is_f64(delay) {
                f64::from_bits(delay as u64)
            } else {
                f64::NAN
            };
            let delay_ms: u64 = if delay_f64.is_nan() || delay_f64.is_sign_negative() {
                0
            } else if delay_f64 > (u32::MAX as f64) {
                u32::MAX as u64
            } else {
                delay_f64 as u64
            };
            let id = {
                let mut next_id = caller
                    .data()
                    .next_timer_id
                    .lock()
                    .expect("next_timer_id mutex");
                let id = *next_id;
                *next_id += 1;
                id
            };
            let deadline = Instant::now() + Duration::from_millis(delay_ms);
            let mut timers = caller.data().timers.lock().expect("timers mutex");
            timers.push(TimerEntry {
                id,
                deadline,
                callback,
                repeating: true,
                interval: Duration::from_millis(delay_ms),
            });
            value::encode_f64(id as f64)
        },
    );
    linker.define(&mut store, "env", "set_interval", f)?;

    // ── Import 30: clear_interval(i64) → () ───────────────────────────────
    let f = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, timer_id: i64| {
            if value::is_f64(timer_id) {
                let id = f64::from_bits(timer_id as u64) as u32;
                caller
                    .data()
                    .cancelled_timers
                    .lock()
                    .expect("cancelled_timers mutex")
                    .insert(id);
            }
            // simplified no-op
        },
    );
    linker.define(&mut store, "env", "clear_interval", f)?;

    // ── Import 31: fetch(i64) → i64 ────────────────────────────────────────
    let f = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, url_val: i64| -> i64 {
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
    linker.define(&mut store, "env", "fetch", f)?;

    // ── Import 32: json_stringify(i64) → i64 ──────────────────────────────
    let f = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            let json_str = runtime_json_stringify(&mut caller, val);
            store_runtime_string(&caller, json_str)
        },
    );
    linker.define(&mut store, "env", "json_stringify", f)?;

    // ── Import 33: json_parse(i64) → i64 ──────────────────────────────────
    let f = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
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
    linker.define(&mut store, "env", "json_parse", f)?;
    // ── Import 34: closure_create(i32, i64) -> i64 ────────────────────────────
    let f = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, func_idx: i32, env_obj: i64| -> i64 {
            let mut closures = caller.data().closures.lock().expect("closures mutex");
            let idx = closures.len() as u32;
            closures.push(ClosureEntry {
                func_idx: func_idx as u32,
                env_obj,
            });
            value::encode_closure_idx(idx)
        },
    );
    linker.define(&mut store, "env", "closure_create", f)?;
    // ── Import 35: closure_get_func(i32) -> i32 ─────────────────────────────
    let f = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, closure_idx: i32| -> i32 {
            let closures = caller.data().closures.lock().expect("closures mutex");
            closures
                .get(closure_idx as usize)
                .map(|e| e.func_idx as i32)
                .unwrap_or(-1)
        },
    );
    linker.define(&mut store, "env", "closure_get_func", f)?;
    // ── Import 36: closure_get_env(i32) -> i64 ─────────────────────────────
    let f = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, closure_idx: i32| -> i64 {
            let closures = caller.data().closures.lock().expect("closures mutex");
            closures
                .get(closure_idx as usize)
                .map(|e| e.env_obj)
                .unwrap_or_else(value::encode_undefined)
        },
    );
    linker.define(&mut store, "env", "closure_get_env", f)?;
    // ── Array method host functions (imports 37-48) ────────────────────
    let f = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let cap = read_array_capacity(&mut caller, ptr).unwrap_or(0);
            let mut ptr = ptr;
            if len >= cap {
                let new_cap = cap.max(1) * 2;
                let needed = (len + 1).max(new_cap);
                if let Some(new_ptr) = grow_array(&mut caller, ptr, arr, needed) {
                    ptr = new_ptr;
                } else {
                    return value::encode_undefined();
                }
            }
            write_array_elem(&mut caller, ptr, len, val);
            write_array_length(&mut caller, ptr, len + 1);
            value::encode_f64((len + 1) as f64)
        },
    );
    linker.define(&mut store, "env", "arr_push", f)?;
    let f = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, arr: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            if len == 0 {
                return value::encode_undefined();
            }
            let new_len = len - 1;
            let val =
                read_array_elem(&mut caller, ptr, new_len).unwrap_or(value::encode_undefined());
            write_array_length(&mut caller, ptr, new_len);
            val
        },
    );
    linker.define(&mut store, "env", "arr_pop", f)?;
    let f = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_bool(false);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                if let Some(elem) = read_array_elem(&mut caller, ptr, i)
                    && elem == val {
                        return value::encode_bool(true);
                    }
            }
            value::encode_bool(false)
        },
    );
    linker.define(&mut store, "env", "arr_includes", f)?;
    let f = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64, from_val: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_f64(-1.0);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            let from_idx = if value::is_f64(from_val) {
                f64::from_bits(from_val as u64) as i32
            } else {
                0
            };
            let start = if from_idx >= 0 {
                (from_idx as usize).min(len as usize)
            } else {
                ((len + from_idx).max(0)) as usize
            };
            for i in start..len as usize {
                if let Some(elem) = read_array_elem(&mut caller, ptr, i as u32)
                    && elem == val {
                        return value::encode_f64(i as f64);
                    }
            }
            value::encode_f64(-1.0)
        },
    );
    linker.define(&mut store, "env", "arr_index_of", f)?;
    let f = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, arr: i64, sep_val: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let sep_str = render_value(&mut caller, sep_val).unwrap_or_else(|_| ",".to_string());
            let mut parts = Vec::new();
            for i in 0..len {
                if let Some(elem) = read_array_elem(&mut caller, ptr, i) {
                    parts.push(render_value(&mut caller, elem).unwrap_or_else(|_| "".to_string()));
                } else {
                    parts.push(String::new());
                }
            }
            store_runtime_string(&caller, parts.join(&sep_str))
        },
    );
    linker.define(&mut store, "env", "arr_join", f)?;
    let f = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, _arr1: i64, _arr2: i64| -> i64 {
            unimplemented!("Array.prototype.concat is not yet implemented in wjsm")
        },
    );
    linker.define(&mut store, "env", "arr_concat", f)?;
    let f = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, _arr: i64, _start: i64, _end: i64| -> i64 {
            unimplemented!("Array.prototype.slice is not yet implemented in wjsm")
        },
    );
    linker.define(&mut store, "env", "arr_slice", f)?;
    let f = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64, _start: i64, _end: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return arr;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                write_array_elem(&mut caller, ptr, i, val);
            }
            arr
        },
    );
    linker.define(&mut store, "env", "arr_fill", f)?;
    let f = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, arr: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return arr;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len / 2 {
                let a = read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let b = read_array_elem(&mut caller, ptr, len - 1 - i)
                    .unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, ptr, i, b);
                write_array_elem(&mut caller, ptr, len - 1 - i, a);
            }
            arr
        },
    );
    linker.define(&mut store, "env", "arr_reverse", f)?;
    let f = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, _arr: i64, _depth: i64| -> i64 {
            unimplemented!("Array.prototype.flat is not yet implemented in wjsm")
        },
    );
    linker.define(&mut store, "env", "arr_flat", f)?;
    let f = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, arr: i64, len_val: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return arr;
            };
            let len = if value::is_f64(len_val) {
                f64::from_bits(len_val as u64) as u32
            } else {
                return arr;
            };
            write_array_length(&mut caller, ptr, len);
            arr
        },
    );
    linker.define(&mut store, "env", "arr_init_length", f)?;
    let f = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, arr: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            value::encode_f64(len as f64)
        },
    );
    linker.define(&mut store, "env", "arr_get_length", f)?;
    Ok(())
}

