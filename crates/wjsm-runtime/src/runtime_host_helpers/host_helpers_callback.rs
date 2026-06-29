use super::*;
fn prepare_callback_shadow_stack(
    caller: &mut Caller<'_, RuntimeState>,
    args: &[i64],
) -> anyhow::Result<(wasmtime::Global, i32)> {
    let env = WasmEnv::from_caller(caller).ok_or_else(|| anyhow::anyhow!("WasmEnv"))?;
    let shadow_sp_global = caller
        .get_export("__shadow_sp")
        .and_then(|e| e.into_global())
        .ok_or_else(|| anyhow::anyhow!("no __shadow_sp"))?;
    let saved_sp = push_args_to_shadow_stack(caller, &env, args)
        .ok_or_else(|| anyhow::anyhow!("shadow stack push failed"))?;
    Ok((shadow_sp_global, saved_sp))
}

fn resolve_handle_val_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    val: i64,
) -> Option<usize> {
    let handle_idx = (val as u64 & 0xFFFF_FFFF) as usize;
    // 函数值低 32 位是函数表索引；其属性对象 handle 从 __function_props_base 起算，需重定位。
    // 与 runtime_values::handle_index_of 保持一致，避免 read/write 漂移。
    let handle_idx = if value::is_function(val) {
        let base = env
            .function_props_base
            .and_then(|g| g.get(&mut *ctx).i32())
            .unwrap_or(0)
            .max(0) as usize;
        handle_idx.saturating_add(base)
    } else {
        handle_idx
    };
    resolve_handle_idx_with_env(ctx, env, handle_idx)
}

/// 走完 proxy apply-trap / target 链，返回最终回调目标。
/// microtask（Store）与 host reentrant（Caller）共用。
/// 与 `resolve_callback_target_with_env` 一致：含 Proxy `apply` trap、bound、closure 等。
pub(crate) fn is_callable_with_env<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    val: i64,
) -> bool {
    if value::is_callable(val) {
        return true;
    }
    resolve_callback_target_with_env(ctx, env, val).is_ok()
}

pub(crate) fn resolve_callback_target_with_env<
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
>(
    ctx: &mut C,
    env: &WasmEnv,
    func_val: i64,
) -> anyhow::Result<CallbackTarget> {
    let mut resolved = func_val;
    loop {
        if value::is_closure(resolved)
            || value::is_function(resolved)
            || value::is_native_callable(resolved)
            || value::is_bound(resolved)
        {
            break;
        }
        if !value::is_proxy(resolved) {
            return Err(anyhow::anyhow!("not callable"));
        }
        let handle = value::decode_proxy_handle(resolved) as usize;
        let entry = {
            let table = ctx
                .state_mut()
                .proxy_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            table.get(handle).cloned()
        };
        let entry = match entry {
            Some(e) => e,
            None => return Err(anyhow::anyhow!("proxy handle not found")),
        };
        if entry.revoked {
            return Err(anyhow::anyhow!("proxy has been revoked"));
        }
        if let Some(handler_ptr) = resolve_handle_val_with_env(ctx, env, entry.handler) {
            let trap = read_object_property_by_name_with_env(ctx, env, handler_ptr, "apply")
                .unwrap_or(value::encode_undefined());
            if !value::is_undefined(trap) && !value::is_null(trap) {
                return Ok(CallbackTarget::ApplyTrap {
                    trap,
                    handler: entry.handler,
                    proxy_target: entry.target,
                });
            }
        }
        resolved = entry.target;
    }
    if value::is_native_callable(resolved) {
        return Ok(CallbackTarget::Native(resolved));
    }
    if value::is_bound(resolved) {
        let bound_idx = value::decode_bound_idx(resolved) as usize;
        let bound = ctx
            .state_mut()
            .bound_objects
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let record = &bound[bound_idx];
        return Ok(CallbackTarget::Bound {
            target_func: record.target_func,
            bound_this: record.bound_this,
            bound_args: record.bound_args.clone(),
        });
    }
    if value::is_closure(resolved) {
        let idx = value::decode_closure_idx(resolved) as usize;
        let closures = ctx
            .state_mut()
            .closures
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = closures
            .get(idx)
            .ok_or_else(|| anyhow::anyhow!("closure index out of range"))?;
        return Ok(CallbackTarget::Wasm {
            func_idx: entry.func_idx,
            env_obj: entry.env_obj,
        });
    }
    if value::is_function(resolved) {
        return Ok(CallbackTarget::Wasm {
            func_idx: value::decode_function_idx(resolved),
            env_obj: value::encode_undefined(),
        });
    }
    Err(anyhow::anyhow!("not callable"))
}

/// WASM 函数表调用的公共前置：swap new_target，返回 `(func, previous_new_target)`。
fn lookup_callback_wasm_func_with_env<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    func_idx: u32,
) -> anyhow::Result<(wasmtime::Func, i64)> {
    let func_ref = env
        .func_table
        .get(&mut *ctx, func_idx as u64)
        .ok_or_else(|| anyhow::anyhow!("table get failed"))?;
    let func = func_ref
        .as_func()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("table entry not a function"))?;
    let previous_new_target = ctx
        .state_mut()
        .new_target
        .swap(value::encode_undefined(), Ordering::Relaxed);
    Ok((*func, previous_new_target))
}

fn dispatch_native_callable_with_env<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    callable: i64,
    _this_val: i64,
    args: &[i64],
) -> Option<i64> {
    if !value::is_native_callable(callable) {
        return None;
    }
    let idx = value::decode_native_callable_idx(callable) as usize;
    let record = {
        let table = ctx
            .state_mut()
            .native_callables
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table.get(idx).cloned()
    }?;
    let argument = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    match record {
        NativeCallable::PromiseResolvingFunction {
            promise,
            already_resolved,
            kind,
        } => {
            let mut already = already_resolved.lock().unwrap_or_else(|e| e.into_inner());
            if *already {
                return Some(value::encode_undefined());
            }
            *already = true;
            drop(already);
            match kind {
                PromiseResolvingKind::Fulfill => {
                    resolve_promise(ctx, env, promise, argument);
                }
                PromiseResolvingKind::Reject => {
                    settle_promise(
                        ctx.state_mut(),
                        promise,
                        PromiseSettlement::Reject(argument),
                    );
                }
            }
            Some(value::encode_undefined())
        }
        NativeCallable::PromiseCombinatorReaction { .. } => Some(value::encode_undefined()),
        _ => None,
    }
}

fn build_args_array_with_env<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    elements: &[i64],
) -> i64 {
    let arr = alloc_array_with_env(ctx, env, elements.len() as u32);
    for (i, &arg) in elements.iter().enumerate() {
        set_array_elem_with_env(ctx, env, arr, i as i32, arg);
    }
    arr
}

async fn invoke_proxy_apply_trap_async<
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
>(
    ctx: &mut C,
    env: &WasmEnv,
    trap: i64,
    handler: i64,
    proxy_target: i64,
    this_val: i64,
    call_args: &[i64],
) -> anyhow::Result<i64> {
    let arr = build_args_array_with_env(ctx, env, call_args);
    let wasm_args = [proxy_target, this_val, arr];
    let sp = push_args_to_shadow_stack(ctx, env, &wasm_args)
        .ok_or_else(|| anyhow::anyhow!("shadow stack push failed"))?;
    let inner = resolve_callback_target_with_env(ctx, env, trap)?;
    Box::pin(dispatch_callback_target_async(
        ctx, env, inner, handler, &wasm_args, sp,
    ))
    .await
}

async fn dispatch_callback_target_async<
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
>(
    ctx: &mut C,
    env: &WasmEnv,
    target: CallbackTarget,
    this_val: i64,
    args: &[i64],
    shadow_sp: i32,
) -> anyhow::Result<i64> {
    match target {
        CallbackTarget::Native(resolved) => {
            restore_shadow_sp(ctx, env, shadow_sp);
            dispatch_native_callable_with_env(ctx, env, resolved, this_val, args)
                .ok_or_else(|| anyhow::anyhow!("native callable not supported in this context"))
        }
        CallbackTarget::ApplyTrap {
            trap,
            handler,
            proxy_target,
        } => {
            restore_shadow_sp(ctx, env, shadow_sp);
            Box::pin(invoke_proxy_apply_trap_async(
                ctx,
                env,
                trap,
                handler,
                proxy_target,
                this_val,
                args,
            ))
            .await
        }
        CallbackTarget::Bound {
            target_func,
            bound_this,
            bound_args,
        } => {
            restore_shadow_sp(ctx, env, shadow_sp);
            let mut combined_args = bound_args;
            combined_args.extend_from_slice(args);
            Box::pin(invoke_resolved_callback_async(
                ctx,
                env,
                target_func,
                bound_this,
                &combined_args,
            ))
            .await
        }
        CallbackTarget::Wasm { func_idx, env_obj } => {
            let (func, previous_new_target) =
                lookup_callback_wasm_func_with_env(ctx, env, func_idx)?;
            let mut results = [Val::I64(0)];
            let call_result = func
                .call_async(
                    &mut *ctx,
                    &[
                        Val::I64(env_obj),
                        Val::I64(this_val),
                        Val::I32(shadow_sp),
                        Val::I32(args.len() as i32),
                    ],
                    &mut results,
                )
                .await;
            ctx.state_mut()
                .new_target
                .store(previous_new_target, Ordering::Relaxed);
            restore_shadow_sp(ctx, env, shadow_sp);
            call_result?;
            Ok(results[0].unwrap_i64())
        }
    }
}

async fn invoke_resolved_callback_async<
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
>(
    ctx: &mut C,
    env: &WasmEnv,
    func_val: i64,
    this_val: i64,
    args: &[i64],
) -> anyhow::Result<i64> {
    let target = resolve_callback_target_with_env(ctx, env, func_val)?;
    match target {
        CallbackTarget::ApplyTrap {
            trap,
            handler,
            proxy_target,
        } => {
            Box::pin(invoke_proxy_apply_trap_async(
                ctx,
                env,
                trap,
                handler,
                proxy_target,
                this_val,
                args,
            ))
            .await
        }
        _ => {
            let shadow_sp = push_args_to_shadow_stack(ctx, env, args)
                .ok_or_else(|| anyhow::anyhow!("shadow stack push failed"))?;
            dispatch_callback_target_async(ctx, env, target, this_val, args, shadow_sp).await
        }
    }
}

pub(crate) async fn invoke_resolved_callback_async_option<
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
>(
    ctx: &mut C,
    env: &WasmEnv,
    func_val: i64,
    this_val: i64,
    args: &[i64],
) -> Option<i64> {
    match invoke_resolved_callback_async(ctx, env, func_val, this_val, args).await {
        Ok(v) => Some(v),
        Err(err) => {
            set_runtime_error(
                ctx.state_mut(),
                format!("host function callback error: {err}"),
            );
            None
        }
    }
}

/// Phase 3 must-convert 之 host reentrant 路径（按 2026-05-31-async-scheduler-implementation-plan.md 审计条目 + 26-async-audit-refactor-design.md）：
/// 为 `call_wasm_callback`（中央 host reentrant 调用点，proxy/define/array 等 13+ callers）添加 async 版本，与现有 sync `call_wasm_callback` 并存。
///
/// 规则：
/// - 严格与 sync 版本并存，供保留的 sync execute 路径继续使用
/// - 所有 bound/closure/proxy 解析逻辑、shadow stack 更新、native callable 短路、结果处理必须 100% 相同
/// - 仅 Wasm invocation（func table dispatch） + 返回值处理完全等价；唯一差异是将 `func.call(...)` 替换为 `func.call_async(...).await`
/// - 本阶段保持调用点不变（runtime_host_helpers 内部递归及所有 host_imports 调用仍使用 sync 版本；未来当 async host fn 路径激活时同步转换调用点）
/// - 精确保留原有行为，无任何语义或顺序差异
///
/// 特别提醒（plan Correction 3 + lib.rs 已有注释 + 审计计划）：
///   在 Store::epoch_deadline_async_yield_and_update 之后，
///   *所有* 经由该 Store 的 Wasm 调用（主 + 回调，包括此处 host reentrant 中的 func table 调用）都必须走 async API（call_async 等）。
///   本文件中的 async 版本即为此准备；sync 版本仅留给未切换的 sync execute 路径。
async fn dispatch_callback_target_caller_async(
    caller: &mut Caller<'_, RuntimeState>,
    env: &WasmEnv,
    target: CallbackTarget,
    this_val: i64,
    args: &[i64],
    shadow_sp: i32,
    shadow_sp_global: wasmtime::Global,
) -> anyhow::Result<i64> {
    match target {
        CallbackTarget::Native(resolved) => {
            let _ = shadow_sp_global.set(&mut *caller, Val::I32(shadow_sp));
            Box::pin(call_native_callable_with_args_from_caller_async(
                caller,
                resolved,
                this_val,
                args.to_vec(),
            ))
            .await
            .ok_or_else(|| anyhow::anyhow!("native callable returned None"))
        }
        CallbackTarget::ApplyTrap {
            trap,
            handler,
            proxy_target,
        } => {
            let _ = shadow_sp_global.set(&mut *caller, Val::I32(shadow_sp));
            let arr = alloc_array(caller, args.len() as u32);
            for (i, &arg) in args.iter().enumerate() {
                set_array_elem(caller, arr, i as i32, arg);
            }
            let wasm_args = [proxy_target, this_val, arr];
            let inner = resolve_callback_target_with_env(caller, env, trap)?;
            Box::pin(dispatch_callback_target_caller_async(
                caller,
                env,
                inner,
                handler,
                &wasm_args,
                shadow_sp,
                shadow_sp_global,
            ))
            .await
        }
        CallbackTarget::Bound {
            target_func,
            bound_this,
            bound_args,
        } => {
            let _ = shadow_sp_global.set(&mut *caller, Val::I32(shadow_sp));
            let mut combined_args = bound_args;
            combined_args.extend_from_slice(args);
            Box::pin(call_wasm_callback_async(
                caller,
                target_func,
                bound_this,
                &combined_args,
            ))
            .await
        }
        CallbackTarget::Wasm { func_idx, env_obj } => {
            let (func, previous_new_target) =
                lookup_callback_wasm_func_with_env(caller, env, func_idx)?;
            let mut results = [Val::I64(0)];
            let call_result = func
                .call_async(
                    &mut *caller,
                    &[
                        Val::I64(env_obj),
                        Val::I64(this_val),
                        Val::I32(shadow_sp),
                        Val::I32(args.len() as i32),
                    ],
                    &mut results,
                )
                .await;
            caller
                .data()
                .new_target
                .store(previous_new_target, Ordering::Relaxed);
            let _ = shadow_sp_global.set(&mut *caller, Val::I32(shadow_sp));
            call_result?;
            Ok(results[0].unwrap_i64())
        }
    }
}

pub(crate) async fn call_wasm_callback_async(
    caller: &mut Caller<'_, RuntimeState>,
    func_val: i64,
    this_val: i64,
    args: &[i64],
) -> anyhow::Result<i64> {
    let env = WasmEnv::from_caller(caller).ok_or_else(|| anyhow::anyhow!("WasmEnv"))?;
    let (shadow_sp_global, shadow_sp) = prepare_callback_shadow_stack(caller, args)?;
    let target = resolve_callback_target_with_env(caller, &env, func_val)?;
    dispatch_callback_target_caller_async(
        caller,
        &env,
        target,
        this_val,
        args,
        shadow_sp,
        shadow_sp_global,
    )
    .await
}
