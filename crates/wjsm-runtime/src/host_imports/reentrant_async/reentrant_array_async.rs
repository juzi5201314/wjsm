use super::*;
async fn sort_compare_async(
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

async fn arr_proto_sort_async_body(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return this_val;
    };
    let len = read_array_length(caller, ptr).unwrap_or(0) as usize;
    if len <= 1 {
        return this_val;
    }
    let mut elems: Vec<i64> = (0..len)
        .map(|i| read_array_elem(caller, ptr, i as u32).unwrap_or(value::encode_undefined()))
        .collect();
    if args_count > 0 && value::is_callable(read_shadow_arg(caller, args_base, 0)) {
        let cmp = read_shadow_arg(caller, args_base, 0);
        for i in 0..elems.len() {
            for j in i + 1..elems.len() {
                if sort_compare_async(caller, cmp, elems[i], elems[j]).await
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
        let mut indexed: Vec<(usize, &i64)> = (0..len).map(|i| (i, &elems[i])).collect();
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
        write_array_elem(caller, ptr, i as u32, elem);
    }
    this_val
}

macro_rules! wrap_array_callback_async {
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

pub(crate) fn define_array_object_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    linker.func_wrap_async(
        "env",
        "arr_proto_sort",
        |mut caller: Caller<'_, RuntimeState>,
         (_env_obj, this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                arr_proto_sort_async_body(&mut caller, this_val, args_base, args_count).await
            })
        },
    )?;

    wrap_array_callback_async!(linker, "arr_proto_for_each", arr_proto_for_each_async);
    wrap_array_callback_async!(linker, "arr_proto_map", arr_proto_map_async);
    wrap_array_callback_async!(linker, "arr_proto_filter", arr_proto_filter_async);
    wrap_array_callback_async!(linker, "arr_proto_reduce", arr_proto_reduce_async);
    wrap_array_callback_async!(
        linker,
        "arr_proto_reduce_right",
        arr_proto_reduce_right_async
    );
    wrap_array_callback_async!(linker, "arr_proto_find", arr_proto_find_async);
    wrap_array_callback_async!(linker, "arr_proto_find_index", arr_proto_find_index_async);
    wrap_array_callback_async!(linker, "arr_proto_some", arr_proto_some_async);
    wrap_array_callback_async!(linker, "arr_proto_every", arr_proto_every_async);
    wrap_array_callback_async!(linker, "arr_proto_flat_map", arr_proto_flat_map_async);

    linker.func_wrap_async(
        "env",
        "array_push_spread",
        |mut caller: Caller<'_, RuntimeState>, (arr, iterable): (i64, i64)| {
            Box::new(async move {
                super::super::array_object::array_push_spread_impl_async(&mut caller, arr, iterable).await
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "func_call",
        |mut caller: Caller<'_, RuntimeState>,
         (func, this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                resolve_and_call_async(&mut caller, func, this_val, args_base, args_count).await
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "func_apply",
        |mut caller: Caller<'_, RuntimeState>, (func, this_val, args_array): (i64, i64, i64)| {
            Box::new(
                async move { func_apply_impl_async(&mut caller, func, this_val, args_array).await },
            )
        },
    )?;

    Ok(())
}

async fn arr_proto_for_each_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = if args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_undefined();
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    for i in 0..len {
        let elem = read_array_elem(caller, ptr, i).unwrap_or(value::encode_undefined());
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

async fn arr_proto_map_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = if args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_undefined();
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    let new_arr = alloc_array(caller, len);
    let Some(new_ptr) = resolve_array_ptr(caller, new_arr) else {
        return value::encode_undefined();
    };
    for i in 0..len {
        let elem = read_array_elem(caller, ptr, i).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        let result = match call_wasm_callback_async(
            caller,
            cb,
            this_arg,
            &[elem, idx_val, this_val],
        )
        .await
        {
            Ok(r) => r,
            Err(_) => value::encode_undefined(),
        };
        write_array_elem(caller, new_ptr, i, result);
    }
    write_array_length(caller, new_ptr, len);
    new_arr
}

async fn arr_proto_filter_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = if args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_undefined();
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    let mut passed: Vec<i64> = Vec::new();
    for i in 0..len {
        let elem = read_array_elem(caller, ptr, i).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        let ok = match call_wasm_callback_async(caller, cb, this_arg, &[elem, idx_val, this_val])
            .await
        {
            Ok(r) => value::is_truthy(r),
            Err(_) => false,
        };
        if ok {
            passed.push(elem);
        }
    }
    let new_arr = alloc_array(caller, passed.len() as u32);
    let Some(new_ptr) = resolve_array_ptr(caller, new_arr) else {
        return value::encode_undefined();
    };
    for (i, elem) in passed.iter().enumerate() {
        write_array_elem(caller, new_ptr, i as u32, *elem);
    }
    write_array_length(caller, new_ptr, passed.len() as u32);
    new_arr
}

async fn arr_proto_reduce_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_undefined();
    };
    let len = read_array_length(caller, ptr).unwrap_or(0) as usize;
    if len == 0 {
        if args_count < 2 {
            *caller
                .data()
                .runtime_error
                .lock()
                .expect("runtime error mutex") =
                Some("TypeError: Reduce of empty array with no initial value".to_string());
            return value::encode_undefined();
        }
        return read_shadow_arg(caller, args_base, 1);
    }
    let mut acc: i64;
    let mut start_idx = 0usize;
    if args_count >= 2 {
        acc = read_shadow_arg(caller, args_base, 1);
    } else {
        acc = read_array_elem(caller, ptr, 0).unwrap_or(value::encode_undefined());
        start_idx = 1;
    }
    for i in start_idx..len {
        let elem = read_array_elem(caller, ptr, i as u32).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        match call_wasm_callback_async(
            caller,
            cb,
            value::encode_undefined(),
            &[acc, elem, idx_val, this_val],
        )
        .await
        {
            Ok(r) => acc = r,
            Err(_) => return value::encode_undefined(),
        }
    }
    acc
}

async fn arr_proto_reduce_right_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_undefined();
    };
    let len = read_array_length(caller, ptr).unwrap_or(0) as i32;
    if len == 0 {
        if args_count < 2 {
            *caller
                .data()
                .runtime_error
                .lock()
                .expect("runtime error mutex") =
                Some("TypeError: Reduce of empty array with no initial value".to_string());
            return value::encode_undefined();
        }
        return read_shadow_arg(caller, args_base, 1);
    }
    let mut acc: i64;
    let mut start_idx = len - 1;
    if args_count >= 2 {
        acc = read_shadow_arg(caller, args_base, 1);
    } else {
        acc = read_array_elem(caller, ptr, start_idx as u32).unwrap_or(value::encode_undefined());
        start_idx = len - 2;
    }
    for i in (0..=start_idx as usize).rev() {
        let elem = read_array_elem(caller, ptr, i as u32).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        match call_wasm_callback_async(
            caller,
            cb,
            value::encode_undefined(),
            &[acc, elem, idx_val, this_val],
        )
        .await
        {
            Ok(r) => acc = r,
            Err(_) => return value::encode_undefined(),
        }
    }
    acc
}

async fn arr_proto_find_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    _args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_undefined();
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    for i in 0..len {
        let elem = read_array_elem(caller, ptr, i).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) = call_wasm_callback_async(
            caller,
            cb,
            value::encode_undefined(),
            &[elem, idx_val, this_val],
        )
        .await
            && value::is_truthy(r)
        {
            return elem;
        }
    }
    value::encode_undefined()
}

async fn arr_proto_find_index_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    _args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_f64(-1.0);
    }
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_f64(-1.0);
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    for i in 0..len {
        let elem = read_array_elem(caller, ptr, i).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) = call_wasm_callback_async(
            caller,
            cb,
            value::encode_undefined(),
            &[elem, idx_val, this_val],
        )
        .await
            && value::is_truthy(r)
        {
            return value::encode_f64(i as f64);
        }
    }
    value::encode_f64(-1.0)
}

async fn arr_proto_some_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    _args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_bool(false);
    }
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_bool(false);
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    for i in 0..len {
        let elem = read_array_elem(caller, ptr, i).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        if let Ok(r) = call_wasm_callback_async(
            caller,
            cb,
            value::encode_undefined(),
            &[elem, idx_val, this_val],
        )
        .await
            && value::is_truthy(r)
        {
            return value::encode_bool(true);
        }
    }
    value::encode_bool(false)
}

async fn arr_proto_every_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    _args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_bool(false);
    }
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_bool(false);
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    for i in 0..len {
        let elem = read_array_elem(caller, ptr, i).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        match call_wasm_callback_async(
            caller,
            cb,
            value::encode_undefined(),
            &[elem, idx_val, this_val],
        )
        .await
        {
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

async fn arr_proto_flat_map_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let cb = read_shadow_arg(caller, args_base, 0);
    if !value::is_callable(cb) {
        return value::encode_undefined();
    }
    let this_arg = if args_count > 1 {
        read_shadow_arg(caller, args_base, 1)
    } else {
        value::encode_undefined()
    };
    let Some(ptr) = resolve_array_ptr(caller, this_val) else {
        return value::encode_undefined();
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    let mut elements: Vec<i64> = Vec::new();
    for i in 0..len {
        let elem = read_array_elem(caller, ptr, i).unwrap_or(value::encode_undefined());
        let idx_val = value::encode_f64(i as f64);
        let mapped = match call_wasm_callback_async(
            caller,
            cb,
            this_arg,
            &[elem, idx_val, this_val],
        )
        .await
        {
            Ok(r) => r,
            Err(_) => continue,
        };
        if value::is_array(mapped) {
            if let Some(mapped_ptr) = resolve_array_ptr(caller, mapped) {
                let mapped_len = read_array_length(caller, mapped_ptr).unwrap_or(0);
                for j in 0..mapped_len {
                    if let Some(inner) = read_array_elem(caller, mapped_ptr, j) {
                        elements.push(inner);
                    }
                }
            }
        } else {
            elements.push(mapped);
        }
    }
    let new_arr = alloc_array(caller, elements.len() as u32);
    let Some(new_ptr) = resolve_array_ptr(caller, new_arr) else {
        return value::encode_undefined();
    };
    for (i, elem) in elements.iter().enumerate() {
        write_array_elem(caller, new_ptr, i as u32, *elem);
    }
    write_array_length(caller, new_ptr, elements.len() as u32);
    new_arr
}

