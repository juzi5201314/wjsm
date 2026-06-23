use anyhow::Result;
use wasmtime::{Caller, Func, Linker};

use crate::*;

pub(crate) fn define_timers_arrays(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    // ── Import 34: closure_create(i64, i64) -> i64 ────────────────────────────
    let f = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, func_ref: i64, env_obj: i64| -> i64 {
            let func_idx = if value::is_function(func_ref) {
                value::decode_function_idx(func_ref)
            } else if value::is_closure(func_ref) {
                let idx = value::decode_closure_idx(func_ref) as usize;
                let closures = caller.data().closures.lock().expect("closures mutex");
                closures.get(idx).map(|e| e.func_idx).unwrap_or(0)
            } else {
                0
            };
            let mut closures = caller.data().closures.lock().expect("closures mutex");
            let idx = closures.len() as u32;
            closures.push(ClosureEntry { func_idx, env_obj });
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
            match super::array_object::push_array_value(&mut caller, arr, val) {
                Ok(()) => {
                    let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                        return value::encode_undefined();
                    };
                    let len = read_array_length(&mut caller, ptr).unwrap_or(0);
                    value::encode_f64(len as f64)
                }
                Err(exc) => exc,
            }
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
            super::array_object::array_includes_from(
                &mut caller,
                ptr,
                len,
                val,
                value::encode_undefined(),
            )
        },
    );
    linker.define(&mut store, "env", "arr_includes", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64, from_val: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_f64(-1.0);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            super::array_object::array_index_of_from(&mut caller, ptr, len, val, from_val)
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
                    parts.push(super::array_object::array_join_element_string(
                        &mut caller,
                        elem,
                    ));
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
        |mut caller: Caller<'_, RuntimeState>, arr1: i64, arr2: i64| -> i64 {
            super::array_object::array_concat_two(&mut caller, arr1, arr2)
        },
    );
    linker.define(&mut store, "env", "arr_concat", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, start: i64, end: i64| -> i64 {
            super::array_object::array_slice_range(&mut caller, arr, start, end)
        },
    );
    linker.define(&mut store, "env", "arr_slice", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64, start: i64, end: i64| -> i64 {
            super::array_object::array_fill_range(&mut caller, arr, val, start, end)
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
        |mut caller: Caller<'_, RuntimeState>, arr: i64, depth: i64| -> i64 {
            super::array_object::array_flat_with_depth(&mut caller, arr, depth)
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
                value::decode_f64(len_val) as u32
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
