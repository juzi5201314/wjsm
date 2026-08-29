use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::property_write::{SetCompletion, SetFailure, SetResult};
use super::runtime::{
    delete_property, fail_dispatch, get_property, get_property_with_receiver, has_property,
    is_constructor_value, object_handle, ordinary_set, property_key, type_error,
};
use crate::{NativeAgentState, NativeCallableKind, NativeProxy};

pub(super) fn dispatch_proxy(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::ProxyCreate => create(ctx, state, args),
        Builtin::ProxyRevocable => revocable(ctx, state, args),
        Builtin::ReflectGet => reflect_get(ctx, state, args),
        Builtin::ReflectSet => reflect_set(ctx, state, args),
        Builtin::ReflectHas => has(ctx, state, args),
        Builtin::ReflectDeleteProperty => reflect_delete(ctx, state, args),
        Builtin::ReflectOwnKeys => reflect_own_keys(ctx, state, args),
        Builtin::ReflectApply => reflect_apply(ctx, state, args),
        Builtin::ReflectConstruct => reflect_construct(ctx, state, args),
        Builtin::ReflectGetPrototypeOf => {
            if let Some(exception) = require_reflect_object(ctx, state, args, "getPrototypeOf") {
                return Some(exception);
            }
            super::object::dispatch_object(ctx, state, Builtin::ObjectGetPrototypeOf, args)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        Builtin::ReflectSetPrototypeOf => reflect_set_prototype(ctx, state, args),
        Builtin::ReflectIsExtensible => {
            if let Some(exception) = require_reflect_object(ctx, state, args, "isExtensible") {
                return Some(exception);
            }
            super::object::dispatch_object(ctx, state, Builtin::ObjectIsExtensible, args)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        Builtin::ReflectPreventExtensions => {
            if let Some(exception) = require_reflect_object(ctx, state, args, "preventExtensions")
            {
                return Some(exception);
            }
            let Some(target) = args.first().copied() else {
                return Some(fail_dispatch(ctx));
            };
            if value::is_proxy(target) {
                prevent_extensions(ctx, state, target)
            } else {
                let result = super::object::dispatch_object(
                    ctx,
                    state,
                    Builtin::ObjectPreventExtensions,
                    args,
                )
                .unwrap_or_else(|| fail_dispatch(ctx));
                if value::is_exception(result) {
                    result
                } else {
                    value::encode_bool(true)
                }
            }
        }
        Builtin::ReflectGetOwnPropertyDescriptor => {
            if let Some(exception) =
                require_reflect_object(ctx, state, args, "getOwnPropertyDescriptor")
            {
                return Some(exception);
            }
            super::object::dispatch_object(ctx, state, Builtin::GetOwnPropDesc, args)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        Builtin::ReflectDefineProperty => {
            if let Some(exception) = require_reflect_object(ctx, state, args, "defineProperty") {
                return Some(exception);
            }
            let [target, key, descriptor] = args else {
                return Some(fail_dispatch(ctx));
            };
            if value::is_proxy(*target) {
                reflect_define_property(ctx, state, *target, *key, *descriptor)
            } else {
                let result =
                    super::object::define_property(ctx, state, &[*target, *key, *descriptor]);
                value::encode_bool(!value::is_exception(result))
            }
        }
        _ => return None,
    })
}

/// Reflect 静态方法（§28.1）步骤 1 的 target 校验：非对象一律 TypeError，
/// 文案对齐 V8 kCalledOnNonObject（`Reflect.<method> called on non-object`）。
/// 返回 `Some(exception)` 表示校验失败。
fn require_reflect_object(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    method: &str,
) -> Option<i64> {
    let target = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if super::runtime::is_language_object(target) {
        return None;
    }
    Some(type_error(
        ctx,
        state,
        &format!("Reflect.{method} called on non-object"),
    ))
}

/// Reflect.ownKeys（§28.1.11）：target.[[OwnPropertyKeys]]() 全量键（含
/// 符号，区别于 Object.getOwnPropertyNames 的 String 过滤）；Proxy 走
/// ownKeys trap。
fn reflect_own_keys(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    if let Some(exception) = require_reflect_object(ctx, state, args, "ownKeys") {
        return exception;
    }
    let Some(target) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let keys = if value::is_proxy(target) {
        match own_keys(ctx, state, target) {
            Ok(keys) => keys,
            Err(exception) => return exception,
        }
    } else {
        let Some(properties) = super::object::own_keys(state, target, false) else {
            return fail_dispatch(ctx);
        };
        properties.into_iter().map(|(key, _)| key).collect()
    };
    state
        .allocate_array_values_with_gc_retry(ctx, &keys)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn is_proxy_target(encoded: i64) -> bool {
    value::is_object(encoded)
        || value::is_array(encoded)
        || value::is_callable(encoded)
        || value::is_proxy(encoded)
}

fn create(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [target, handler] = args else {
        return fail_dispatch(ctx);
    };
    if !is_proxy_target(*target) || !is_proxy_target(*handler) {
        return super::runtime::type_error(ctx, state, "Proxy target and handler must be objects");
    }
    let handle = match state.proxy_free.pop() {
        Some(handle) => handle,
        None => {
            let Ok(handle) = u32::try_from(state.proxies.len()) else {
                return fail_dispatch(ctx);
            };
            state.proxies.push(None);
            handle
        }
    };
    state.proxies[handle as usize] = Some(NativeProxy {
        target: *target,
        handler: *handler,
        revoked: false,
    });
    let proxy = value::encode_proxy_handle(handle);
    for stored in [proxy, *target, *handler] {
        state.gc.record_host_write(proxy, None, Some(stored));
    }
    proxy
}

fn revocable(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let proxy = create(ctx, state, args);
    if value::is_exception(proxy) {
        return proxy;
    }
    let proxy_handle = value::decode_proxy_handle(proxy);
    let Some(revoke) = state.native_callable(NativeCallableKind::ProxyRevoke(proxy_handle)) else {
        return fail_dispatch(ctx);
    };
    let Ok(result) = state.allocate_object_with_gc_retry(ctx, 2, false) else {
        return fail_dispatch(ctx);
    };
    let result_handle = value::decode_handle(result);
    for (name, stored) in [("proxy", proxy), ("revoke", revoke)] {
        let Some(key) = state.intern_property_string(name.into()) else {
            return fail_dispatch(ctx);
        };
        if state
            .gc
            .heap()
            .set_property(result_handle, key, stored as u64)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    result
}

/// [[Construct]] 存在性在 ProxyCreate（§10.5.14）时按 target 确定，revoke
/// 只清空 [[ProxyTarget]]/[[ProxyHandler]] 的可用性、不移除内部方法：
/// IsConstructor(revokedProxy) 仍为 true，调用期才抛 revoked TypeError。
pub(super) fn is_constructor_proxy(state: &NativeAgentState, encoded: i64) -> bool {
    raw_entry(state, encoded).is_some_and(|proxy| is_constructor_value(state, proxy.target))
}

/// IsArray（§7.2.2）步骤 3 的 Proxy 穿透：沿 [[ProxyTarget]] 链解包判定；
/// revoked proxy 返回 None（调用方按规范抛 TypeError）。
pub(super) fn is_array_target(state: &NativeAgentState, encoded: i64) -> Option<bool> {
    let mut current = encoded;
    loop {
        if value::is_array(current) {
            return Some(true);
        }
        if !value::is_proxy(current) {
            return Some(false);
        }
        current = entry(state, current)?.target;
    }
}

fn entry(state: &NativeAgentState, encoded: i64) -> Option<NativeProxy> {
    raw_entry(state, encoded).filter(|entry| !entry.revoked)
}

/// 含 revoked 的原始记录：只用于创建时即确定、revoke 后不消失的能力判定
/// （[[Construct]] 存在性）与需要区分 revoked 文案的入口。
fn raw_entry(state: &NativeAgentState, encoded: i64) -> Option<NativeProxy> {
    if !value::is_proxy(encoded) {
        return None;
    }
    usize::try_from(value::decode_proxy_handle(encoded))
        .ok()
        .and_then(|handle| state.proxies.get(handle))
        .and_then(|proxy| proxy.as_ref())
        .copied()
}

fn require_entry(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    encoded: i64,
) -> Result<NativeProxy, i64> {
    entry(state, encoded).ok_or_else(|| {
        super::runtime::type_error(ctx, state, "Cannot perform operation on a revoked Proxy")
    })
}

fn target_descriptor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target: i64,
    key: i64,
) -> Result<Option<super::object::PropertyDescriptor>, i64> {
    let descriptor = super::object::get_own_property_descriptor(ctx, state, &[target, key]);
    if value::is_exception(descriptor) {
        return Err(descriptor);
    }
    if value::is_undefined(descriptor) {
        return Ok(None);
    }
    let Some(handle) = object_handle(descriptor) else {
        return Err(fail_dispatch(ctx));
    };
    super::object::read_descriptor(ctx, state, handle).map(Some)
}

fn target_is_extensible(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target: i64,
) -> Result<bool, i64> {
    let result = if value::is_proxy(target) {
        is_extensible(ctx, state, target)
    } else {
        super::object::dispatch_object(ctx, state, Builtin::ObjectIsExtensible, &[target])
            .unwrap_or_else(|| fail_dispatch(ctx))
    };
    if value::is_exception(result) {
        Err(result)
    } else {
        Ok(super::runtime::is_truthy(state, result))
    }
}

fn proxy_invariant_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    message: &str,
) -> i64 {
    super::runtime::type_error(ctx, state, message)
}

fn trap(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handler: i64,
    name: &str,
) -> Result<Option<i64>, i64> {
    let Some(key) = state.intern_property_string(name.into()) else {
        return Err(fail_dispatch(ctx));
    };
    match get_property(
        ctx,
        state,
        handler,
        crate::dispatch::runtime::encoded_property_key(key),
    ) {
        Ok(trap) if value::is_undefined(trap) => Ok(None),
        Ok(trap) if value::is_callable(trap) => Ok(Some(trap)),
        Ok(_) | Err(()) => Err(fail_dispatch(ctx)),
    }
}
pub(crate) fn own_keys(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    proxy: i64,
) -> Result<Vec<i64>, i64> {
    let entry = require_entry(ctx, state, proxy)?;
    let trap_keys = match trap(ctx, state, entry.handler, "ownKeys")? {
        Some(trap) => {
            let result = state
                .invoke_callable(ctx, trap, entry.handler, &[entry.target])
                .ok_or_else(|| fail_dispatch(ctx))?;
            if value::is_exception(result) {
                return Err(result);
            }
            array_arguments(state, result).ok_or_else(|| {
                proxy_invariant_error(ctx, state, "Proxy ownKeys trap must return an object")
            })?
        }
        None => {
            return if value::is_proxy(entry.target) {
                own_keys(ctx, state, entry.target)
            } else {
                super::object::own_keys(state, entry.target, false)
                    .map(|properties| properties.into_iter().map(|(key, _)| key).collect())
                    .ok_or_else(|| fail_dispatch(ctx))
            };
        }
    };

    let mut trap_key_ids = Vec::with_capacity(trap_keys.len());
    for key in &trap_keys {
        if !value::is_string(*key) && !value::is_symbol(*key) {
            return Err(proxy_invariant_error(
                ctx,
                state,
                "Proxy ownKeys trap result must contain only strings and symbols",
            ));
        }
        let Some(key_id) = property_key(state, *key) else {
            return Err(fail_dispatch(ctx));
        };
        if trap_key_ids.contains(&key_id) {
            return Err(proxy_invariant_error(
                ctx,
                state,
                "Proxy ownKeys trap returned duplicate entries",
            ));
        }
        trap_key_ids.push(key_id);
    }

    let target_keys = if value::is_proxy(entry.target) {
        own_keys(ctx, state, entry.target)?
    } else {
        super::object::own_keys(state, entry.target, false)
            .map(|properties| properties.into_iter().map(|(key, _)| key).collect())
            .ok_or_else(|| fail_dispatch(ctx))?
    };
    let target_extensible = target_is_extensible(ctx, state, entry.target)?;
    for target_key in &target_keys {
        let Some(target_key_id) = property_key(state, *target_key) else {
            return Err(fail_dispatch(ctx));
        };
        let present = trap_key_ids.contains(&target_key_id);
        let descriptor = target_descriptor(ctx, state, entry.target, *target_key)?;
        let non_configurable =
            descriptor.is_some_and(|descriptor| descriptor.configurable == Some(false));
        if !present && (non_configurable || !target_extensible) {
            return Err(proxy_invariant_error(
                ctx,
                state,
                "Proxy ownKeys invariant violated by omitted target property",
            ));
        }
    }
    if !target_extensible && trap_keys.len() != target_keys.len() {
        return Err(proxy_invariant_error(
            ctx,
            state,
            "Proxy ownKeys invariant violated by an extra property on a non-extensible target",
        ));
    }
    Ok(trap_keys)
}

pub(crate) fn get_own_property_descriptor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    proxy: i64,
    key: i64,
) -> i64 {
    let entry = match require_entry(ctx, state, proxy) {
        Ok(entry) => entry,
        Err(exception) => return exception,
    };
    let trap_result = match trap(ctx, state, entry.handler, "getOwnPropertyDescriptor") {
        Ok(Some(trap)) => state
            .invoke_callable(ctx, trap, entry.handler, &[entry.target, key])
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Ok(None) => {
            return super::object::get_own_property_descriptor(ctx, state, &[entry.target, key]);
        }
        Err(exception) => return exception,
    };
    if value::is_exception(trap_result) {
        return trap_result;
    }
    let target_descriptor = match target_descriptor(ctx, state, entry.target, key) {
        Ok(descriptor) => descriptor,
        Err(exception) => return exception,
    };
    let target_extensible = match target_is_extensible(ctx, state, entry.target) {
        Ok(extensible) => extensible,
        Err(exception) => return exception,
    };
    if value::is_undefined(trap_result) {
        return match target_descriptor {
            None => value::encode_undefined(),
            Some(descriptor) if descriptor.configurable == Some(false) => proxy_invariant_error(
                ctx,
                state,
                "Proxy getOwnPropertyDescriptor invariant violated: non-configurable target property cannot be reported as missing",
            ),
            Some(_) if !target_extensible => proxy_invariant_error(
                ctx,
                state,
                "Proxy getOwnPropertyDescriptor invariant violated: target is non-extensible and property cannot be reported as missing",
            ),
            Some(_) => value::encode_undefined(),
        };
    }
    let Some(trap_descriptor_handle) = object_handle(trap_result) else {
        return proxy_invariant_error(
            ctx,
            state,
            "Proxy getOwnPropertyDescriptor trap must return an object or undefined",
        );
    };
    let trap_descriptor = match super::object::read_descriptor(ctx, state, trap_descriptor_handle) {
        Ok(descriptor) => descriptor,
        Err(exception) => return exception,
    };
    match target_descriptor {
        None if !target_extensible => {
            return proxy_invariant_error(
                ctx,
                state,
                "Proxy getOwnPropertyDescriptor invariant violated: cannot add a property to a non-extensible target",
            );
        }
        None if trap_descriptor.configurable == Some(false) => {
            return proxy_invariant_error(
                ctx,
                state,
                "Proxy getOwnPropertyDescriptor invariant violated: non-configurable descriptor is incompatible with target",
            );
        }
        Some(current)
            if !super::object::descriptor_is_compatible(state, trap_descriptor, current) =>
        {
            return proxy_invariant_error(
                ctx,
                state,
                "Proxy getOwnPropertyDescriptor invariant violated: descriptor is incompatible with target",
            );
        }
        Some(current)
            if trap_descriptor.configurable == Some(false)
                && current.configurable == Some(true) =>
        {
            return proxy_invariant_error(
                ctx,
                state,
                "Proxy getOwnPropertyDescriptor invariant violated: non-configurable descriptor is incompatible with target",
            );
        }
        _ => {}
    }
    super::object::complete_descriptor_object(ctx, state, trap_descriptor)
}

pub(crate) fn is_extensible(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    proxy: i64,
) -> i64 {
    let entry = match require_entry(ctx, state, proxy) {
        Ok(entry) => entry,
        Err(exception) => return exception,
    };
    let target_extensible = match target_is_extensible(ctx, state, entry.target) {
        Ok(extensible) => extensible,
        Err(exception) => return exception,
    };
    let trap_result = match trap(ctx, state, entry.handler, "isExtensible") {
        Ok(Some(trap)) => state
            .invoke_callable(ctx, trap, entry.handler, &[entry.target])
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Ok(None) => return value::encode_bool(target_extensible),
        Err(exception) => return exception,
    };
    if value::is_exception(trap_result) {
        return trap_result;
    }
    let trap_extensible = super::runtime::is_truthy(state, trap_result);
    if trap_extensible != target_extensible {
        return proxy_invariant_error(
            ctx,
            state,
            "Proxy isExtensible trap returned result that does not match target's extensibility",
        );
    }
    value::encode_bool(trap_extensible)
}

pub(crate) fn prevent_extensions(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    proxy: i64,
) -> i64 {
    let entry = match require_entry(ctx, state, proxy) {
        Ok(entry) => entry,
        Err(exception) => return exception,
    };
    let trap_result = match trap(ctx, state, entry.handler, "preventExtensions") {
        Ok(Some(trap)) => state
            .invoke_callable(ctx, trap, entry.handler, &[entry.target])
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Ok(None) => {
            let result = super::object::dispatch_object(
                ctx,
                state,
                Builtin::ObjectPreventExtensions,
                &[entry.target],
            )
            .unwrap_or_else(|| fail_dispatch(ctx));
            return if value::is_exception(result) {
                result
            } else {
                value::encode_bool(true)
            };
        }
        Err(exception) => return exception,
    };
    if value::is_exception(trap_result) {
        return trap_result;
    }
    let succeeded = super::runtime::is_truthy(state, trap_result);
    if succeeded {
        match target_is_extensible(ctx, state, entry.target) {
            Ok(false) => {}
            Ok(true) => {
                return proxy_invariant_error(
                    ctx,
                    state,
                    "Proxy preventExtensions trap returned true, but target remains extensible",
                );
            }
            Err(exception) => return exception,
        }
    }
    value::encode_bool(succeeded)
}

pub(crate) fn define_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    proxy: i64,
    key: i64,
    descriptor: i64,
) -> i64 {
    match try_define_property(ctx, state, proxy, key, descriptor) {
        Ok(true) => proxy,
        Ok(false) => {
            // V8 falsish 文案（与 set / deleteProperty trap 同款式）。
            let key_text = super::runtime::render_value(state, key);
            let message = format!(
                "'defineProperty' on proxy: trap returned falsish for property '{key_text}'"
            );
            proxy_invariant_error(ctx, state, &message)
        }
        Err(exception) => exception,
    }
}

fn reflect_define_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    proxy: i64,
    key: i64,
    descriptor: i64,
) -> i64 {
    match try_define_property(ctx, state, proxy, key, descriptor) {
        Ok(succeeded) => value::encode_bool(succeeded),
        Err(exception) => exception,
    }
}

fn try_define_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    proxy: i64,
    key: i64,
    descriptor: i64,
) -> Result<bool, i64> {
    let entry = require_entry(ctx, state, proxy)?;
    let Some(descriptor_handle) = object_handle(descriptor) else {
        return Err(fail_dispatch(ctx));
    };
    let descriptor_record = super::object::read_descriptor(ctx, state, descriptor_handle)?;
    let Some(trap) = trap(ctx, state, entry.handler, "defineProperty")? else {
        let result = super::object::define_property(ctx, state, &[entry.target, key, descriptor]);
        return Ok(!value::is_exception(result));
    };
    let result = state
        .invoke_callable(ctx, trap, entry.handler, &[entry.target, key, descriptor])
        .ok_or_else(|| fail_dispatch(ctx))?;
    if value::is_exception(result) {
        return Err(result);
    }
    if !super::runtime::is_truthy(state, result) {
        return Ok(false);
    }

    let target_descriptor = target_descriptor(ctx, state, entry.target, key)?;
    let target_extensible = target_is_extensible(ctx, state, entry.target)?;
    match target_descriptor {
        None if !target_extensible => {
            return Err(proxy_invariant_error(
                ctx,
                state,
                "Proxy defineProperty invariant violated: target is non-extensible",
            ));
        }
        None if descriptor_record.configurable == Some(false) => {
            return Err(proxy_invariant_error(
                ctx,
                state,
                "Proxy defineProperty invariant violated: cannot create a non-configurable target property",
            ));
        }
        Some(current)
            if !super::object::descriptor_is_compatible(state, descriptor_record, current) =>
        {
            return Err(proxy_invariant_error(
                ctx,
                state,
                "Proxy defineProperty invariant violated: descriptor is incompatible with target",
            ));
        }
        Some(current)
            if descriptor_record.configurable == Some(false)
                && current.configurable == Some(true) =>
        {
            return Err(proxy_invariant_error(
                ctx,
                state,
                "Proxy defineProperty invariant violated: cannot report a configurable property as non-configurable",
            ));
        }
        _ => {}
    }
    Ok(true)
}
pub(crate) fn get_prototype(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    proxy: i64,
) -> i64 {
    let entry = match require_entry(ctx, state, proxy) {
        Ok(entry) => entry,
        Err(exception) => return exception,
    };
    match trap(ctx, state, entry.handler, "getPrototypeOf") {
        Ok(Some(trap)) => {
            let result = state
                .invoke_callable(ctx, trap, entry.handler, &[entry.target])
                .unwrap_or_else(|| fail_dispatch(ctx));
            // trap 抛出的异常原样上抛（§10.5.1 步骤 7 的 ? Call）。
            if value::is_exception(result) {
                return result;
            }
            // 步骤 8：返回值既非 Object 也非 null 抛 TypeError（callable /
            // Proxy / RegExp 都是 Object）。
            if value::is_null(result)
                || value::is_object(result)
                || value::is_array(result)
                || value::is_callable(result)
                || value::is_proxy(result)
                || value::is_regexp(result)
            {
                result
            } else {
                type_error(
                    ctx,
                    state,
                    "'getPrototypeOf' on proxy: trap returned neither object nor null",
                )
            }
        }
        Ok(None) => super::object::dispatch_object(
            ctx,
            state,
            Builtin::ObjectGetPrototypeOf,
            &[entry.target],
        )
        .unwrap_or_else(|| fail_dispatch(ctx)),
        Err(exception) => exception,
    }
}

pub(crate) fn set_prototype(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    proxy: i64,
    prototype: i64,
) -> i64 {
    let entry = match require_entry(ctx, state, proxy) {
        Ok(entry) => entry,
        Err(exception) => return exception,
    };
    match trap(ctx, state, entry.handler, "setPrototypeOf") {
        Ok(Some(trap)) => state
            .invoke_callable(ctx, trap, entry.handler, &[entry.target, prototype])
            .map(|result| value::encode_bool(super::runtime::is_truthy(state, result)))
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Ok(None) => {
            let result = super::object::dispatch_object(
                ctx,
                state,
                Builtin::ObjectSetPrototypeOf,
                &[entry.target, prototype],
            )
            .unwrap_or_else(|| fail_dispatch(ctx));
            value::encode_bool(!value::is_exception(result))
        }
        Err(exception) => exception,
    }
}

pub(crate) fn reflect_set_prototype(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [target, prototype] = args else {
        return fail_dispatch(ctx);
    };
    // Reflect.setPrototypeOf（§28.1.14）步骤 1：非对象 target TypeError（区别
    // 于 Object.setPrototypeOf 对基元的原样放行）。
    if let Some(exception) = require_reflect_object(ctx, state, args, "setPrototypeOf") {
        return exception;
    }
    // 步骤 2：proto 非对象非 null 必须抛 TypeError——不能混入下方「[[SetPrototypeOf]]
    // 失败返回 false」的异常吞并（循环原型 / 不可扩展按规范返回 false）。
    if !(value::is_null(*prototype) || super::runtime::is_language_object(*prototype)) {
        let rendered = super::runtime::render_value(state, *prototype);
        return type_error(
            ctx,
            state,
            &format!("Object prototype may only be an Object or null: {rendered}"),
        );
    }
    if value::is_proxy(*target) {
        return set_prototype(ctx, state, *target, *prototype);
    }
    let result = super::object::dispatch_object(ctx, state, Builtin::ObjectSetPrototypeOf, args)
        .unwrap_or_else(|| fail_dispatch(ctx));
    value::encode_bool(!value::is_exception(result))
}

pub(super) fn get(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    proxy: i64,
    key: i64,
    receiver: i64,
) -> i64 {
    let entry = match require_entry(ctx, state, proxy) {
        Ok(entry) => entry,
        Err(exception) => return exception,
    };
    match trap(ctx, state, entry.handler, "get") {
        Ok(Some(trap)) => state
            .invoke_callable(ctx, trap, entry.handler, &[entry.target, key, receiver])
            .unwrap_or_else(|| fail_dispatch(ctx)),
        // §10.5.8 步骤 6：无 trap 时委托 target.[[Get]](P, Receiver)，
        // Receiver 保持原值（链上 getter / __proto__ 访问器的 this 是 proxy）。
        Ok(None) => get_property_with_receiver(ctx, state, entry.target, key, receiver)
            .unwrap_or_else(|()| fail_dispatch(ctx)),
        Err(exception) => exception,
    }
}

/// proxy 的 [[Set]]（§10.5.9）：trap 返回 falsish 记为规范失败（strict 抛
/// TypeError 与否由赋值点决定，Reflect.set 返回 false）；无 trap 时委托
/// target 的 [[Set]] 并原样传递其完成结果。
pub(super) fn set(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    proxy: i64,
    key: i64,
    stored: i64,
    receiver: i64,
) -> SetResult {
    let entry = require_entry(ctx, state, proxy)?;
    match trap(ctx, state, entry.handler, "set") {
        Ok(Some(trap)) => {
            let result = state
                .invoke_callable(
                    ctx,
                    trap,
                    entry.handler,
                    &[entry.target, key, stored, receiver],
                )
                .unwrap_or_else(|| fail_dispatch(ctx));
            if value::is_exception(result) {
                Err(result)
            } else if super::runtime::is_truthy(state, result) {
                Ok(SetCompletion::Written)
            } else {
                Ok(SetCompletion::Failed(SetFailure::ProxyFalsish))
            }
        }
        Ok(None) if value::is_object(entry.target) => {
            ordinary_set(ctx, state, entry.target, key, stored, receiver)
        }
        Ok(None) => set_plain(ctx, state, entry.target, key, stored, receiver),
        Err(exception) => Err(exception),
    }
}

fn set_plain(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target: i64,
    key: i64,
    stored: i64,
    receiver: i64,
) -> SetResult {
    if value::is_proxy(target) {
        return set(ctx, state, target, key, stored, receiver);
    }
    let Some(key_id) = property_key(state, key) else {
        return Err(fail_dispatch(ctx));
    };
    if value::is_callable(target) {
        // callable target 的完整 [[Set]]：链上 setter / 可写性与可扩展性
        // 拒绝均需生效（frozen 函数经无 trap proxy 赋值不得绕过）。
        return super::callable_chain::set_with_receiver(
            ctx, state, target, key_id, stored, receiver,
        );
    }
    if value::is_array(target) {
        return super::property_write::set_array_named_property(ctx, state, target, key_id, stored);
    }
    let Some(handle) = object_handle(target) else {
        return Err(fail_dispatch(ctx));
    };
    state
        .gc
        .heap()
        .set_property(handle, key_id, stored as u64)
        .map(|()| SetCompletion::Written)
        .map_err(|_| fail_dispatch(ctx))
}

fn set_descriptor(state: &mut NativeAgentState, stored: i64, create: bool) -> Option<i64> {
    let fields = [
        ("value", stored),
        ("writable", value::encode_bool(true)),
        ("enumerable", value::encode_bool(true)),
        ("configurable", value::encode_bool(true)),
    ];
    let field_count = if create { fields.len() } else { 1 };
    let descriptor = state.allocate_object(field_count as u32, false).ok()?;
    for &(name, field) in &fields[..field_count] {
        let key = state.intern_property_string(name.into())?;
        state
            .gc
            .heap()
            .set_property(value::decode_handle(descriptor), key, field as u64)
            .ok()?;
    }
    Some(descriptor)
}

pub(super) fn set_receiver_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    key: i64,
    stored: i64,
) -> SetResult {
    let current = get_own_property_descriptor(ctx, state, receiver, key);
    if value::is_exception(current) {
        return Err(current);
    }
    let create = value::is_undefined(current);
    if !create {
        let handle = object_handle(current).ok_or_else(|| fail_dispatch(ctx))?;
        let descriptor = super::object::read_descriptor(ctx, state, handle)?;
        if descriptor.is_accessor() {
            return Ok(SetCompletion::Failed(SetFailure::GetterOnly));
        }
        if descriptor.writable == Some(false) {
            return Ok(SetCompletion::Failed(SetFailure::ReadOnly));
        }
    }
    let descriptor = set_descriptor(state, stored, create).ok_or_else(|| fail_dispatch(ctx))?;
    // defineProperty trap 返回 falsish 同属 proxy 写失败口径。
    match try_define_property(ctx, state, receiver, key, descriptor)? {
        true => Ok(SetCompletion::Written),
        false => Ok(SetCompletion::Failed(SetFailure::ProxyFalsish)),
    }
}

fn reflect_get(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [target, key, rest @ ..] = args else {
        return fail_dispatch(ctx);
    };
    // Reflect.get（§28.1.5）step 2：先 ToPropertyKey 再 [[Get]]；
    // `super[k]` 成员读复用本入口，对象键在此完成用户转换再入。
    let key = &match super::runtime::to_property_key_value(ctx, state, *key) {
        Ok(key) => key,
        Err(exception) => return exception,
    };
    let receiver = rest.first().copied().unwrap_or(*target);
    if value::is_proxy(*target) {
        get(ctx, state, *target, *key, receiver)
    } else {
        get_property_with_receiver(ctx, state, *target, *key, receiver)
            .unwrap_or_else(|()| fail_dispatch(ctx))
    }
}

fn reflect_set(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [target, key, stored, rest @ ..] = args else {
        return fail_dispatch(ctx);
    };
    // Reflect.set（§28.1.9）step 2：先 ToPropertyKey 再 [[Set]]；
    // `super[k] = v` 成员写复用本入口，对象键在此完成用户转换再入。
    let key = &match super::runtime::to_property_key_value(ctx, state, *key) {
        Ok(key) => key,
        Err(exception) => return exception,
    };
    let receiver = rest.first().copied().unwrap_or(*target);
    let completion = if value::is_proxy(*target) {
        set(ctx, state, *target, *key, *stored, receiver)
    } else {
        ordinary_set(ctx, state, *target, *key, *stored, receiver)
    };
    match completion {
        Ok(completion) => value::encode_bool(completion.succeeded()),
        Err(exception) => exception,
    }
}

pub(crate) fn has(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [target, key] = args else {
        return fail_dispatch(ctx);
    };
    if !value::is_proxy(*target) {
        return match has_property(ctx, state, *target, *key) {
            Ok(present) => value::encode_bool(present),
            Err(exception) => exception,
        };
    }
    let entry = match require_entry(ctx, state, *target) {
        Ok(entry) => entry,
        Err(exception) => return exception,
    };
    let trap_result = match trap(ctx, state, entry.handler, "has") {
        Ok(Some(trap)) => state
            .invoke_callable(ctx, trap, entry.handler, &[entry.target, *key])
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Ok(None) => return has(ctx, state, &[entry.target, *key]),
        Err(exception) => return exception,
    };
    if value::is_exception(trap_result) {
        return trap_result;
    }
    let present = super::runtime::is_truthy(state, trap_result);
    if !present {
        let descriptor = match target_descriptor(ctx, state, entry.target, *key) {
            Ok(descriptor) => descriptor,
            Err(exception) => return exception,
        };
        if let Some(descriptor) = descriptor {
            let extensible = match target_is_extensible(ctx, state, entry.target) {
                Ok(extensible) => extensible,
                Err(exception) => return exception,
            };
            if descriptor.configurable == Some(false) || !extensible {
                return proxy_invariant_error(
                    ctx,
                    state,
                    "Proxy has invariant violated: target property cannot be hidden",
                );
            }
        }
    }
    value::encode_bool(present)
}

fn reflect_delete(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [target, key] = args else {
        return fail_dispatch(ctx);
    };
    proxy_delete_with_mode(ctx, state, *target, *key, false)
}

/// delete 操作符（§13.5.5.9）作用于 proxy 的 [[Delete]] 入口：strict 位
/// 沿 target 链透传，falsish trap 在 strict 下抛 V8 口径的 proxy 专属
/// TypeError；Reflect.deleteProperty 走 strict=false 的同一核心。
pub(super) fn delete_for_operator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    proxy: i64,
    key: i64,
    strict: bool,
) -> i64 {
    proxy_delete_with_mode(ctx, state, proxy, key, strict)
}

fn proxy_delete_with_mode(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target: i64,
    key: i64,
    strict: bool,
) -> i64 {
    if !value::is_proxy(target) {
        // 非 proxy 收口：strict 失败升级 TypeError 由 runtime 侧统一渲染。
        if strict {
            return super::runtime::delete_property_operator_with_key(
                ctx, state, target, key, strict,
            );
        }
        return delete_property(state, target, key)
            .map(value::encode_bool)
            .unwrap_or_else(|()| fail_dispatch(ctx));
    }
    let entry = match require_entry(ctx, state, target) {
        Ok(entry) => entry,
        Err(exception) => return exception,
    };
    let trap_result = match trap(ctx, state, entry.handler, "deleteProperty") {
        Ok(Some(trap)) => state
            .invoke_callable(ctx, trap, entry.handler, &[entry.target, key])
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Ok(None) => return proxy_delete_with_mode(ctx, state, entry.target, key, strict),
        Err(exception) => return exception,
    };
    if value::is_exception(trap_result) {
        return trap_result;
    }
    let deleted = super::runtime::is_truthy(state, trap_result);
    if deleted {
        let descriptor = match target_descriptor(ctx, state, entry.target, key) {
            Ok(descriptor) => descriptor,
            Err(exception) => return exception,
        };
        if let Some(descriptor) = descriptor {
            let extensible = match target_is_extensible(ctx, state, entry.target) {
                Ok(extensible) => extensible,
                Err(exception) => return exception,
            };
            if descriptor.configurable == Some(false) || !extensible {
                return proxy_invariant_error(
                    ctx,
                    state,
                    "Proxy deleteProperty invariant violated: target property cannot be deleted",
                );
            }
        }
    } else if strict {
        // §13.5.5.9 步骤 5.d：strict 下 deleteStatus 为 false 抛 TypeError，
        // trap 返回 falsish 时取 V8 的 proxy 专属消息。
        let message = format!(
            "'deleteProperty' on proxy: trap returned falsish for property '{}'",
            super::runtime::render_value(state, key)
        );
        return super::runtime::type_error(ctx, state, &message);
    }
    value::encode_bool(deleted)
}

fn array_arguments(state: &NativeAgentState, encoded: i64) -> Option<Vec<i64>> {
    if !value::is_array(encoded) {
        return None;
    }
    let handle = value::decode_handle(encoded);
    let length = state.gc.heap().array_length(handle).ok()?;
    (0..length)
        .map(|index| {
            state
                .gc
                .heap()
                .get_element(handle, index)
                .ok()
                .flatten()
                .map(|stored| stored as i64)
        })
        .collect()
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    proxy: i64,
    this_value: i64,
    arguments: &[i64],
) -> i64 {
    // §10.5.12 步骤 1–2：handler 为 null（revoked）抛 TypeError，文案对齐
    // V8（含内部方法名 'apply'）。
    let Some(entry) = raw_entry(state, proxy) else {
        return fail_dispatch(ctx);
    };
    if entry.revoked {
        return super::runtime::type_error(
            ctx,
            state,
            "Cannot perform 'apply' on a proxy that has been revoked",
        );
    }
    let Some(args_array) = state
        .allocate_array_values_with_gc_retry(ctx, arguments)
        .ok()
    else {
        return fail_dispatch(ctx);
    };
    match trap(ctx, state, entry.handler, "apply") {
        Ok(Some(trap)) => state
            .invoke_callable(
                ctx,
                trap,
                entry.handler,
                &[entry.target, this_value, args_array],
            )
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Ok(None) => state
            .invoke_callable(ctx, entry.target, this_value, arguments)
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Err(exception) => exception,
    }
}

pub(crate) fn construct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    proxy: i64,
    arguments: &[i64],
    new_target: i64,
) -> i64 {
    // §10.5.13 步骤 1–2：handler 为 null（revoked）抛 TypeError，文案对齐
    // V8（含内部方法名 'construct'）。
    let Some(entry) = raw_entry(state, proxy) else {
        return fail_dispatch(ctx);
    };
    if entry.revoked {
        return super::runtime::type_error(
            ctx,
            state,
            "Cannot perform 'construct' on a proxy that has been revoked",
        );
    }
    let Some(args_array) = state
        .allocate_array_values_with_gc_retry(ctx, arguments)
        .ok()
    else {
        return fail_dispatch(ctx);
    };
    match trap(ctx, state, entry.handler, "construct") {
        Ok(Some(trap)) => {
            let result = state
                .invoke_callable(
                    ctx,
                    trap,
                    entry.handler,
                    &[entry.target, args_array, new_target],
                )
                .unwrap_or_else(|| fail_dispatch(ctx));
            if value::is_exception(result) || value::is_js_object(result) {
                result
            } else {
                super::runtime::type_error(ctx, state, "Proxy construct trap must return an object")
            }
        }
        Ok(None) => reflect_construct(ctx, state, &[entry.target, args_array, new_target]),
        Err(exception) => exception,
    }
}

fn reflect_apply(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [target, this_value, arguments] = args else {
        return fail_dispatch(ctx);
    };
    let Some(arguments) = array_arguments(state, *arguments) else {
        return fail_dispatch(ctx);
    };
    state
        .invoke_callable(ctx, *target, *this_value, &arguments)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn reflect_construct(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [target, arguments, rest @ ..] = args else {
        return fail_dispatch(ctx);
    };
    let new_target = rest.first().copied().unwrap_or(*target);
    if !is_constructor_value(state, *target) || !is_constructor_value(state, new_target) {
        // 文案对齐 V8/Node：callable 按 Function.prototype.toString 形态渲染
        // （"function max() { [native code] } is not a constructor"）。
        let culprit = if is_constructor_value(state, *target) {
            new_target
        } else {
            *target
        };
        let rendered = if value::is_callable(culprit) {
            state
                .callable_to_string_source(culprit)
                .unwrap_or_else(|| super::runtime::render_value(state, culprit))
        } else {
            super::runtime::render_value(state, culprit)
        };
        let message = format!("{rendered} is not a constructor");
        return super::runtime::type_error(ctx, state, &message);
    }
    let Some(arguments) = array_arguments(state, *arguments) else {
        return fail_dispatch(ctx);
    };
    super::runtime::construct_value(ctx, state, *target, &arguments, new_target)
}
