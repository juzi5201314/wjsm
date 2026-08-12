use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{
    delete_property, fail_dispatch, get_property, get_property_with_receiver, has_property,
    object_handle, ordinary_set, property_key,
};
use crate::{ASSIGNED_PROPERTY_FLAGS, NativeAgentState, NativeCallableKind, NativeProxy};

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
        Builtin::ReflectOwnKeys => {
            super::object::dispatch_object(ctx, state, Builtin::ObjectGetOwnPropertyNames, args)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        Builtin::ReflectApply => reflect_apply(ctx, state, args),
        Builtin::ReflectConstruct => reflect_construct(ctx, state, args),
        Builtin::ReflectGetPrototypeOf => {
            super::object::dispatch_object(ctx, state, Builtin::ObjectGetPrototypeOf, args)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        Builtin::ReflectSetPrototypeOf => reflect_set_prototype(ctx, state, args),
        Builtin::ReflectIsExtensible => {
            super::object::dispatch_object(ctx, state, Builtin::ObjectIsExtensible, args)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        Builtin::ReflectPreventExtensions => {
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
            super::object::dispatch_object(ctx, state, Builtin::GetOwnPropDesc, args)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        Builtin::ReflectDefineProperty => {
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
    let Ok(handle) = u32::try_from(state.proxies.len()) else {
        return fail_dispatch(ctx);
    };
    state.proxies.push(NativeProxy {
        target: *target,
        handler: *handler,
        revoked: false,
    });
    value::encode_proxy_handle(handle)
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
    let Ok(result) = state.allocate_object(2, false) else {
        return fail_dispatch(ctx);
    };
    let result_handle = value::decode_handle(result);
    for (name, stored) in [("proxy", proxy), ("revoke", revoke)] {
        let Some(key) = state.intern_text(name.into(), value::TAG_STRING) else {
            return fail_dispatch(ctx);
        };
        if state
            .heap
            .set_property(result_handle, value::decode_handle(key), stored as u64)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    result
}

fn entry(state: &NativeAgentState, encoded: i64) -> Option<NativeProxy> {
    if !value::is_proxy(encoded) {
        return None;
    }
    usize::try_from(value::decode_proxy_handle(encoded))
        .ok()
        .and_then(|handle| state.proxies.get(handle))
        .copied()
        .filter(|entry| !entry.revoked)
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
    let Some(key) = state.intern_text(name.into(), value::TAG_STRING) else {
        return Err(fail_dispatch(ctx));
    };
    match get_property(ctx, state, handler, key) {
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
        Ok(false) => proxy_invariant_error(ctx, state, "Proxy defineProperty trap returned false"),
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
            if value::is_null(result)
                || value::is_object(result)
                || value::is_array(result)
                || value::is_function(result)
                || value::is_proxy(result)
            {
                result
            } else {
                fail_dispatch(ctx)
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
        Ok(None) => {
            get_property(ctx, state, entry.target, key).unwrap_or_else(|()| fail_dispatch(ctx))
        }
        Err(exception) => exception,
    }
}

pub(super) fn set(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    proxy: i64,
    key: i64,
    stored: i64,
    receiver: i64,
) -> i64 {
    let entry = match require_entry(ctx, state, proxy) {
        Ok(entry) => entry,
        Err(exception) => return exception,
    };
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
                result
            } else if super::runtime::is_truthy(state, result) {
                stored
            } else {
                fail_dispatch(ctx)
            }
        }
        Ok(None) if value::is_object(entry.target) => {
            match ordinary_set(ctx, state, entry.target, key, stored, receiver) {
                Ok(true) => stored,
                Ok(false) => super::runtime::type_error(
                    ctx,
                    state,
                    "Proxy target property cannot be assigned",
                ),
                Err(exception) => exception,
            }
        }
        Ok(None) => set_plain(ctx, state, entry.target, key, stored),
        Err(exception) => exception,
    }
}

fn set_plain(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target: i64,
    key: i64,
    stored: i64,
) -> i64 {
    if value::is_proxy(target) {
        return set(ctx, state, target, key, stored, target);
    }
    let Some(key_id) = property_key(state, key) else {
        return fail_dispatch(ctx);
    };
    if value::is_callable(target) {
        state.callable_properties.insert((target, key_id), stored);
        state
            .callable_property_flags
            .entry((target, key_id))
            .or_insert(ASSIGNED_PROPERTY_FLAGS);
        return stored;
    }
    if value::is_array(target) {
        let handle = value::decode_handle(target);
        state.note_array_property(handle, key_id);
        state.array_properties.insert((handle, key_id), stored);
        return stored;
    }
    let Some(handle) = object_handle(target) else {
        return fail_dispatch(ctx);
    };
    state
        .heap
        .set_property(handle, key_id, stored as u64)
        .map(|()| stored)
        .unwrap_or_else(|_| fail_dispatch(ctx))
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
        let key = state.intern_text(name.into(), value::TAG_STRING)?;
        state
            .heap
            .set_property(
                value::decode_handle(descriptor),
                value::decode_handle(key),
                field as u64,
            )
            .ok()?;
    }
    Some(descriptor)
}

pub(crate) fn set_receiver_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    key: i64,
    stored: i64,
) -> Result<bool, i64> {
    let current = get_own_property_descriptor(ctx, state, receiver, key);
    if value::is_exception(current) {
        return Err(current);
    }
    let create = value::is_undefined(current);
    if !create {
        let handle = object_handle(current).ok_or_else(|| fail_dispatch(ctx))?;
        let descriptor = super::object::read_descriptor(ctx, state, handle)?;
        if descriptor.is_accessor() || descriptor.writable == Some(false) {
            return Ok(false);
        }
    }
    let descriptor = set_descriptor(state, stored, create).ok_or_else(|| fail_dispatch(ctx))?;
    try_define_property(ctx, state, receiver, key, descriptor)
}

fn reflect_get(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [target, key, rest @ ..] = args else {
        return fail_dispatch(ctx);
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
    let receiver = rest.first().copied().unwrap_or(*target);
    if value::is_proxy(*target) {
        let result = set(ctx, state, *target, *key, *stored, receiver);
        if value::is_exception(result) {
            result
        } else {
            value::encode_bool(true)
        }
    } else {
        match ordinary_set(ctx, state, *target, *key, *stored, receiver) {
            Ok(success) => value::encode_bool(success),
            Err(exception) => exception,
        }
    }
}

pub(crate) fn has(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [target, key] = args else {
        return fail_dispatch(ctx);
    };
    if !value::is_proxy(*target) {
        return value::encode_bool(has_property(state, *target, *key));
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
    if !value::is_proxy(*target) {
        return delete_property(state, *target, *key)
            .map(value::encode_bool)
            .unwrap_or_else(|()| fail_dispatch(ctx));
    }
    let entry = match require_entry(ctx, state, *target) {
        Ok(entry) => entry,
        Err(exception) => return exception,
    };
    let trap_result = match trap(ctx, state, entry.handler, "deleteProperty") {
        Ok(Some(trap)) => state
            .invoke_callable(ctx, trap, entry.handler, &[entry.target, *key])
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Ok(None) => return reflect_delete(ctx, state, &[entry.target, *key]),
        Err(exception) => return exception,
    };
    if value::is_exception(trap_result) {
        return trap_result;
    }
    let deleted = super::runtime::is_truthy(state, trap_result);
    if deleted {
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
                    "Proxy deleteProperty invariant violated: target property cannot be deleted",
                );
            }
        }
    }
    value::encode_bool(deleted)
}

fn array_arguments(state: &NativeAgentState, encoded: i64) -> Option<Vec<i64>> {
    if !value::is_array(encoded) {
        return None;
    }
    let handle = value::decode_handle(encoded);
    let length = state.heap.array_length(handle).ok()?;
    (0..length)
        .map(|index| {
            state
                .heap
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
    let entry = match require_entry(ctx, state, proxy) {
        Ok(entry) => entry,
        Err(exception) => return exception,
    };
    let Some(args_array) = state.allocate_array_values(arguments).ok() else {
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
    let entry = match require_entry(ctx, state, proxy) {
        Ok(entry) => entry,
        Err(exception) => return exception,
    };
    let Some(args_array) = state.allocate_array_values(arguments).ok() else {
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
            if value::is_exception(result) {
                result
            } else if value::is_js_object(result) {
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
    let Some(arguments) = array_arguments(state, *arguments) else {
        return fail_dispatch(ctx);
    };
    if value::is_proxy(*target) {
        return construct(ctx, state, *target, &arguments, new_target);
    }
    let Ok(this_value) = state.allocate_object(4, false) else {
        return fail_dispatch(ctx);
    };
    let Some(prototype_key) = state.intern_text("prototype".into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    let prototype =
        get_property(ctx, state, new_target, prototype_key).unwrap_or_else(|()| fail_dispatch(ctx));
    if value::is_exception(prototype) {
        return prototype;
    }
    if let Some(prototype) = object_handle(prototype)
        && state
            .heap
            .set_prototype(value::decode_handle(this_value), prototype)
            .is_err()
    {
        return fail_dispatch(ctx);
    }
    let result = state
        .invoke_constructor(ctx, *target, new_target, this_value, &arguments)
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_js_object(result) {
        result
    } else {
        this_value
    }
}
