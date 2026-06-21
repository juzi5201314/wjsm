use super::*;
use crate::host_imports::typedarray_new_methods::{sab_read, sab_write, ta_read, ta_resolve, ta_write};
async fn typedarray_sort_compare_async(
    caller: &mut Caller<'_, RuntimeState>,
    cmp: i64,
    a: i64,
    b: i64,
) -> std::cmp::Ordering {
    let result = call_wasm_callback_async(caller, cmp, value::encode_undefined(), &[a, b])
        .await
        .unwrap_or(value::encode_f64(0.0));
    let v = value::decode_f64(result);
    if v > 0.0 {
        std::cmp::Ordering::Greater
    } else if v < 0.0 {
        std::cmp::Ordering::Less
    } else {
        std::cmp::Ordering::Equal
    }
}

async fn typedarray_proto_sort_async_body(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return this_val,
        };
    if length <= 1 {
        return this_val;
    }
    let mut elems: Vec<i64> = (0..length)
        .map(|i| {
            if is_shared {
                sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
            } else {
                ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
            }
            .unwrap_or(value::encode_undefined())
        })
        .collect();
    if args_count > 0 && value::is_callable(read_shadow_arg(caller, args_base, 0)) {
        let cmp = read_shadow_arg(caller, args_base, 0);
        for i in 0..elems.len() {
            for j in i + 1..elems.len() {
                if typedarray_sort_compare_async(caller, cmp, elems[i], elems[j]).await
                    == std::cmp::Ordering::Greater
                {
                    elems.swap(i, j);
                }
            }
        }
    } else {
        let keys: Vec<String> = elems
            .iter()
            .map(|e| render_value(caller, *e).unwrap_or_default())
            .collect();
        let mut indexed: Vec<(usize, &i64)> =
            (0..length as usize).map(|i| (i, &elems[i])).collect();
        indexed.sort_by(|(ia, _), (ib, _)| {
            let ka = &keys[*ia];
            let kb = &keys[*ib];
            let cmp = ka.cmp(kb);
            if cmp == std::cmp::Ordering::Equal {
                ia.cmp(ib)
            } else {
                cmp
            }
        });
        elems = indexed.iter().map(|(_, e)| **e).collect();
    }
    for (i, &elem) in elems.iter().enumerate() {
        if is_shared {
            sab_write(
                caller,
                buf_handle,
                byte_offset,
                elem_size,
                element_kind,
                i as u32,
                elem,
            );
        } else {
            ta_write(
                caller,
                buf_handle,
                byte_offset,
                elem_size,
                element_kind,
                i as u32,
                elem,
            );
        };
    }
    this_val
}

async fn typedarray_proto_for_each_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_undefined(),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = if args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    for i in 0..length {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        if call_wasm_callback_async(caller, cb, this_arg, &[elem, idx_val, this_val])
            .await
            .is_err()
        {
            return value::encode_undefined();
        }
    }
    value::encode_undefined()
}

async fn typedarray_proto_map_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_undefined(),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = if args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    let new_arr = alloc_array(caller, length);
    let Some(arr_ptr) = resolve_array_ptr(caller, new_arr) else {
        return value::encode_undefined();
    };
    for i in 0..length {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        let mapped = match call_wasm_callback_async(
            caller,
            cb,
            this_arg,
            &[elem, idx_val, this_val],
        )
        .await
        {
            Ok(v) => v,
            Err(_) => return value::encode_undefined(),
        };
        write_array_elem(caller, arr_ptr, i, mapped);
    }
    write_array_length(caller, arr_ptr, length);
    new_arr
}

async fn typedarray_proto_filter_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_undefined(),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = if args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    let mut results = Vec::new();
    for i in 0..length {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        let keep = match call_wasm_callback_async(caller, cb, this_arg, &[elem, idx_val, this_val])
            .await
        {
            Ok(v) => value::is_truthy(v),
            Err(_) => return value::encode_undefined(),
        };
        if keep {
            results.push(elem);
        }
    }
    let new_arr = alloc_array(caller, results.len() as u32);
    let Some(arr_ptr) = resolve_array_ptr(caller, new_arr) else {
        return value::encode_undefined();
    };
    for (j, elem) in results.iter().enumerate() {
        write_array_elem(caller, arr_ptr, j as u32, *elem);
    }
    write_array_length(caller, arr_ptr, results.len() as u32);
    new_arr
}

async fn typedarray_proto_reduce_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_undefined(),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let has_init = args_count > 1;
    let init = if has_init {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    if length == 0 && !has_init {
        return value::encode_undefined();
    }
    let mut acc = if has_init {
        init
    } else {
        if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, 0)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, 0)
        }
        .unwrap_or(value::encode_undefined())
    };
    let start = if has_init { 0 } else { 1 };
    for i in start..length {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        acc = match call_wasm_callback_async(
            caller,
            cb,
            value::encode_undefined(),
            &[acc, elem, idx_val, this_val],
        )
        .await
        {
            Ok(v) => v,
            Err(_) => return value::encode_undefined(),
        };
    }
    acc
}

async fn typedarray_proto_reduce_right_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_undefined(),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let has_init = args_count > 1;
    let init = if has_init {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    if length == 0 && !has_init {
        return value::encode_undefined();
    }
    let mut acc = if has_init {
        init
    } else {
        if is_shared {
            sab_read(
                caller,
                buf_handle,
                byte_offset,
                elem_size,
                element_kind,
                length - 1,
            )
        } else {
            ta_read(
                caller,
                buf_handle,
                byte_offset,
                elem_size,
                element_kind,
                length - 1,
            )
        }
        .unwrap_or(value::encode_undefined())
    };
    let end = if has_init {
        length as i32 - 1
    } else {
        length as i32 - 2
    };
    for i in (0..=end as u32).rev() {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        acc = match call_wasm_callback_async(
            caller,
            cb,
            value::encode_undefined(),
            &[acc, elem, idx_val, this_val],
        )
        .await
        {
            Ok(v) => v,
            Err(_) => return value::encode_undefined(),
        };
    }
    acc
}

async fn typedarray_proto_find_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    _args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_undefined(),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = if _args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    for i in 0..length {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) =
            call_wasm_callback_async(caller, cb, this_arg, &[elem, idx_val, this_val]).await
            && value::is_truthy(r)
        {
            return elem;
        }
    }
    value::encode_undefined()
}

async fn typedarray_proto_find_index_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    _args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_f64(-1.0),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_f64(-1.0);
    }
    let this_arg = if _args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    for i in 0..length {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) =
            call_wasm_callback_async(caller, cb, this_arg, &[elem, idx_val, this_val]).await
            && value::is_truthy(r)
        {
            return value::encode_f64(i as f64);
        }
    }
    value::encode_f64(-1.0)
}

async fn typedarray_proto_some_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    _args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_bool(false),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_bool(false);
    }
    let this_arg = if _args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    for i in 0..length {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) =
            call_wasm_callback_async(caller, cb, this_arg, &[elem, idx_val, this_val]).await
            && value::is_truthy(r)
        {
            return value::encode_bool(true);
        }
    }
    value::encode_bool(false)
}

async fn typedarray_proto_every_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    _args_count: i32,
) -> i64 {
    let (buf_handle, byte_offset, length, elem_size, element_kind, is_shared) =
        match ta_resolve(caller, this_val) {
            Some(v) => v,
            None => return value::encode_bool(true),
        };
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_bool(true);
    }
    let this_arg = if _args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    for i in 0..length {
        let elem = if is_shared {
            sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        } else {
            ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, i)
        }
        .unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        match call_wasm_callback_async(caller, cb, this_arg, &[elem, idx_val, this_val]).await {
            Ok(r) => {
                if !value::is_truthy(r) {
                    return value::encode_bool(false);
                }
            }
            Err(_) => return value::encode_bool(false),
        }
    }
    value::encode_bool(true)
}

macro_rules! wrap_typedarray_callback_async {
    ($linker:expr, $name:expr, $body:expr) => {
        $linker.func_wrap_async(
            "env",
            $name,
            |mut caller: Caller<'_, RuntimeState>,
             (_env_obj, this_val, args_base, args_count): (i64, i64, i32, i32)| {
                Box::new(async move { $body(&mut caller, this_val, args_base, args_count).await })
            },
        )?;
    };
}

pub(crate) fn define_typedarray_new_methods_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    linker.func_wrap_async(
        "env",
        "typedarray_proto_sort",
        |mut caller: Caller<'_, RuntimeState>,
         (_env_obj, this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                typedarray_proto_sort_async_body(&mut caller, this_val, args_base, args_count).await
            })
        },
    )?;

    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_for_each",
        typedarray_proto_for_each_async
    );
    wrap_typedarray_callback_async!(linker, "typedarray_proto_map", typedarray_proto_map_async);
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_filter",
        typedarray_proto_filter_async
    );
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_reduce",
        typedarray_proto_reduce_async
    );
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_reduce_right",
        typedarray_proto_reduce_right_async
    );
    wrap_typedarray_callback_async!(linker, "typedarray_proto_find", typedarray_proto_find_async);
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_find_index",
        typedarray_proto_find_index_async
    );
    wrap_typedarray_callback_async!(linker, "typedarray_proto_some", typedarray_proto_some_async);
    wrap_typedarray_callback_async!(
        linker,
        "typedarray_proto_every",
        typedarray_proto_every_async
    );

    Ok(())
}
