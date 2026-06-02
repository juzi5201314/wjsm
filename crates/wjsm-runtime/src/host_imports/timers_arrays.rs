use anyhow::Result;
use wasmtime::{Caller, Linker};

use crate::*;

pub(crate) fn define_timers_arrays(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    // ── Import 32: json_stringify(i64, i64, i64) → i64 (val, replacer, space) ──
    // Current minimal impl only uses val; replacer/space ignored for compatibility.
    // This matches the signature emitted by backend for JSON.stringify calls with args.
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64, _replacer: i64, _space: i64| -> i64 {
            let json_str = runtime_json_stringify(&mut caller, val);
            store_runtime_string(&caller, json_str)
        },
    );
    linker.define(&mut store, "env", "json_stringify", f)?;

    // ── Import 34: closure_create(i32, i64) -> i64 ────────────────────────────
    let f = Func::wrap(
        &mut store,
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
    linker.define(&mut store, "env", "closure_create", f)?;
    // ── Import 35: closure_get_func(i32) -> i32 ─────────────────────────────
    let f = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, closure_idx: i32| -> i32 {
            let closures = caller.data().closures.lock().expect("closures mutex");
            closures
                .get(closure_idx as usize)
                .map(|e| e.func_idx as i32)
                .unwrap_or(-1)
        },
    );
    linker.define(&mut store, "env", "closure_get_func", f)?;
    // ── Import 36: closure_get_env(i32) -> i64 ─────────────────────────────
    let f = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, closure_idx: i32| -> i64 {
            let closures = caller.data().closures.lock().expect("closures mutex");
            closures
                .get(closure_idx as usize)
                .map(|e| e.env_obj)
                .unwrap_or_else(value::encode_undefined)
        },
    );
    linker.define(&mut store, "env", "closure_get_env", f)?;
    // ── Array method host functions (imports 37-48) ────────────────────
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64| -> i64 {
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
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64| -> i64 {
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
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_bool(false);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                if let Some(elem) = read_array_elem(&mut caller, ptr, i)
                    && elem == val
                {
                    return value::encode_bool(true);
                }
            }
            value::encode_bool(false)
        },
    );
    linker.define(&mut store, "env", "arr_includes", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64, from_val: i64| -> i64 {
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
                    && elem == val
                {
                    return value::encode_f64(i as f64);
                }
            }
            value::encode_f64(-1.0)
        },
    );
    linker.define(&mut store, "env", "arr_index_of", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, sep_val: i64| -> i64 {
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
    let f = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _arr1: i64, _arr2: i64| -> i64 {
            unimplemented!("Array.prototype.concat is not yet implemented in wjsm")
        },
    );
    linker.define(&mut store, "env", "arr_concat", f)?;
    let f = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _arr: i64, _start: i64, _end: i64| -> i64 {
            unimplemented!("Array.prototype.slice is not yet implemented in wjsm")
        },
    );
    linker.define(&mut store, "env", "arr_slice", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64, _start: i64, _end: i64| -> i64 {
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
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64| -> i64 {
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
    let f = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _arr: i64, _depth: i64| -> i64 {
            unimplemented!("Array.prototype.flat is not yet implemented in wjsm")
        },
    );
    linker.define(&mut store, "env", "arr_flat", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, len_val: i64| -> i64 {
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
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64| -> i64 {
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
