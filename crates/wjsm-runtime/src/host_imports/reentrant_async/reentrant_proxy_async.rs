use super::*;
pub(crate) async fn proxy_trap_call_trap_with_args_async(
    caller: &mut Caller<'_, RuntimeState>,
    trap: i64,
    this_val: i64,
    args: &[i64],
) -> i64 {
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .unwrap();
    let shadow_sp_global = caller
        .get_export("__shadow_sp")
        .and_then(|e| e.into_global())
        .unwrap();
    let saved_sp = shadow_sp_global.get(&mut *caller).i32().unwrap();
    let total_size = (args.len() * 8) as i32;
    let new_sp = saved_sp + total_size;
    for (i, &arg) in args.iter().enumerate() {
        memory
            .write(
                &mut *caller,
                (saved_sp + i as i32 * 8) as usize,
                &arg.to_le_bytes(),
            )
            .unwrap();
    }
    shadow_sp_global
        .set(&mut *caller, Val::I32(new_sp))
        .unwrap();
    let result = resolve_and_call_async(caller, trap, this_val, saved_sp, args.len() as i32).await;
    shadow_sp_global
        .set(&mut *caller, Val::I32(saved_sp))
        .unwrap();
    result
}

async fn proxy_trap_ordinary_get_by_name_id_async(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    name_id: i32,
) -> i64 {
    if value::is_proxy(target) {
        return Box::pin(proxy_trap_internal_get_async(caller, target, name_id)).await;
    }
    let Some(ptr) = resolve_handle(caller, target) else {
        return value::encode_undefined();
    };
    read_object_property_by_name_id(caller, ptr, name_id as u32)
        .unwrap_or_else(value::encode_undefined)
}

async fn proxy_trap_ordinary_set_by_name_id_async(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    name_id: i32,
    val: i64,
) -> bool {
    if value::is_proxy(target) {
        Box::pin(proxy_trap_internal_set_async(caller, target, name_id, val)).await;
        return true;
    }
    let Some(ptr) = resolve_handle(caller, target) else {
        return false;
    };
    write_object_property_by_name_id(
        caller,
        ptr,
        target,
        name_id as u32,
        val,
        constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE,
    );
    true
}

pub(crate) async fn proxy_trap_internal_get_async(
    caller: &mut Caller<'_, RuntimeState>,
    proxy: i64,
    name_id: i32,
) -> i64 {
    let (target, handler) = match proxy_trap_proxy_entry(caller, proxy, "get") {
        Ok(pair) => pair,
        Err(exc) => return exc,
    };
    if let Some(trap) = proxy_trap_handler_trap(caller, handler, "get") {
        let prop = proxy_trap_property_key_value(caller, name_id);
        return proxy_trap_call_trap_with_args_async(caller, trap, handler, &[target, prop, proxy])
            .await;
    }
    Box::pin(proxy_trap_ordinary_get_by_name_id_async(
        caller, target, name_id,
    ))
    .await
}

pub(crate) async fn proxy_trap_internal_set_async(
    caller: &mut Caller<'_, RuntimeState>,
    proxy: i64,
    name_id: i32,
    val: i64,
) {
    // 注意：set 内部方法返回 void（$obj_set 为 Type 9 `(i64,i32,i64)->()`），无法回传
    // TAG_EXCEPTION，故撤销代理上的 `proxy.x = v` 维持延迟（不可捕获）报错。规范要求的
    // 可捕获 [[Set]] 抛出经 Reflect.set（返回 i64，见 proxy_reflect_async）覆盖。
    let (target, handler) = match proxy_trap_proxy_entry(caller, proxy, "set") {
        Ok(pair) => pair,
        Err(_exc) => {
            set_runtime_error(
                caller.data(),
                "TypeError: Cannot perform 'set' on a proxy that has been revoked".to_string(),
            );
            return;
        }
    };
    if let Some(trap) = proxy_trap_handler_trap(caller, handler, "set") {
        let prop = proxy_trap_property_key_value(caller, name_id);
        let _ = proxy_trap_call_trap_with_args_async(
            caller,
            trap,
            handler,
            &[target, prop, val, proxy],
        )
        .await;
        return;
    }
    let _ = Box::pin(proxy_trap_ordinary_set_by_name_id_async(
        caller, target, name_id, val,
    ))
    .await;
}

pub(crate) async fn proxy_trap_internal_delete_async(
    caller: &mut Caller<'_, RuntimeState>,
    proxy: i64,
    name_id: i32,
) -> i64 {
    let (target, handler) = match proxy_trap_proxy_entry(caller, proxy, "deleteProperty") {
        Ok(pair) => pair,
        Err(exc) => return exc,
    };
    if let Some(trap) = proxy_trap_handler_trap(caller, handler, "deleteProperty") {
        let prop = proxy_trap_property_key_value(caller, name_id);
        let result =
            proxy_trap_call_trap_with_args_async(caller, trap, handler, &[target, prop]).await;
        return value::encode_bool(nanbox_to_bool(result));
    }
    value::encode_bool(true)
}

pub(crate) fn define_proxy_traps_async(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    linker.func_wrap_async(
        "env",
        "proxy_trap_get",
        |mut caller: Caller<'_, RuntimeState>, (proxy, name_id): (i64, i32)| {
            Box::new(
                async move { proxy_trap_internal_get_async(&mut caller, proxy, name_id).await },
            )
        },
    )?;

    linker.func_wrap_async(
        "env",
        "proxy_trap_set",
        |mut caller: Caller<'_, RuntimeState>, (proxy, name_id, val): (i64, i32, i64)| {
            Box::new(async move {
                proxy_trap_internal_set_async(&mut caller, proxy, name_id, val).await;
            })
        },
    )?;

    linker.func_wrap_async(
        "env",
        "proxy_trap_delete",
        |mut caller: Caller<'_, RuntimeState>, (proxy, name_id): (i64, i32)| {
            Box::new(
                async move { proxy_trap_internal_delete_async(&mut caller, proxy, name_id).await },
            )
        },
    )?;

    Ok(())
}

// ── TypedArray async callback overrides ──────────────────────────────────
