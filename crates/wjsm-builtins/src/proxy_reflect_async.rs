//! Proxy trap async 覆盖 + Reflect async 算法（后端无关）。
//!
//! `Reflect.get/set/apply/construct/getPrototypeOf/setPrototypeOf/ownKeys`、
//! `Proxy.apply/construct` 的 Proxy 感知路径在此。host-wasm 闭包缩减为
use wjsm_host::{ExecContext, ProxyEntry, Value};
use wjsm_ir::value;

use crate::proxy_reflect::{reflect_get_own_property_descriptor_impl, reflect_own_keys_impl};
use crate::proxy_traps::{
    proxy_trap_handler_trap, proxy_trap_proxy_entry, proxy_trap_property_key_value,
};

// ── Proxy trap 检查 ──────────────────────────────────────────────────────

/// D4: 检查 proxy 是否已撤销，返回 Some(exception) 如果已撤销，否则 None。
pub fn check_proxy_revoked<E: ExecContext>(ctx: &mut E, entry: &ProxyEntry, op: &str) -> Option<Value> {
    if ctx.proxy_is_revoked(value::decode_proxy_handle(entry.target) as u32) {
        Some(ctx.make_type_error(&format!(
            "Cannot perform '{}' on a proxy that has been revoked",
            op
        )))
    } else {
        None
    }
}

/// 从 ProxyEntry 读取并返回 target / handler 的异常版本（统一入口）。
fn proxy_target_handler<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    op: &str,
) -> Result<(Value, Value), Value> {
    let entry = ctx
        .proxy_entry(value::decode_proxy_handle(target) as u32)
        .ok_or_else(|| {
            ctx.make_type_error(&format!(
                "TypeError: Proxy internal method {op} called on non-proxy"
            ))
        })?;
    if ctx.proxy_is_revoked(value::decode_proxy_handle(target) as u32) {
        return Err(ctx.make_type_error(&format!(
            "TypeError: Cannot perform '{op}' on a proxy that has been revoked"
        )));
    }
    Ok((entry.target, entry.handler))
}

// ── Reflect.has (async, Proxy-aware) ──────────────────────────────────────

/// `Reflect.has(target, prop)` 异步路径（含 Proxy has trap）。
pub async fn reflect_has_async<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    prop: Value,
) -> Value {
    if !value::is_js_object(target) && !value::is_array(target) && !value::is_function(target) {
        return value::encode_bool(false);
    }
    // Proxy has trap
    if value::is_proxy(target) {
        let (t, handler) = match proxy_target_handler(ctx, target, "has") {
            Ok(pair) => pair,
            Err(exc) => return exc,
        };
        if let Some(trap) = proxy_trap_handler_trap(ctx, handler, "has") {
            let result = match ctx.call_js_async(trap, handler, &[t, prop]).await {
                Ok(v) => v,
                Err(_) => return value::encode_bool(false),
            };
            return value::encode_bool(value::is_truthy(result));
        }
        // 无 trap → target 的 OrdinaryHasProperty
        return Box::pin(reflect_has_async(ctx, t, prop)).await;
    }
    // 非 proxy：委托同步实现
    crate::proxy_reflect::reflect_has_impl(ctx, target, prop)
}

// ── Reflect.get (async, Proxy-aware) ──────────────────────────────────────

/// `Reflect.get(target, prop, receiver)` 异步路径（含 Proxy get trap）。
///
/// 完全异步实现：不调用 `reflect_get_sync`（后者用 `block_in_place`，
/// 在 current-thread tokio runtime 上会 panic）。对普通对象走 OrdinaryGet
/// 算法：沿原型链查找属性槽，accessor 属性异步调用 getter。
pub async fn reflect_get_impl_with_receiver_async<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    prop: Value,
    receiver: Value,
) -> Value {
    if value::is_proxy(target) {
        let (t, handler) = match proxy_trap_proxy_entry(ctx, target, "get") {
            Ok(pair) => pair,
            Err(exc) => return exc,
        };
        if let Some(trap) = proxy_trap_handler_trap(ctx, handler, "get") {
            return match ctx
                .call_js_async(trap, handler, &[t, prop, receiver])
                .await
            {
                Ok(v) => v,
                Err(_) => value::encode_undefined(),
            };
        }
        // 无 trap → 走 target 的 OrdinaryGet
        return ordinary_get_async(ctx, t, prop, receiver).await;
    }
    ordinary_get_async(ctx, target, prop, receiver).await
}

/// §10.1.8 OrdinaryGet 异步实现：沿原型链查找属性槽，accessor 异步调用 getter。
async fn ordinary_get_async<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    prop: Value,
    receiver: Value,
) -> Value {
    let prop_str = if value::is_string(prop) {
        ctx.value_to_key_string(prop).ok()
    } else {
        None
    };

    // NativeCallable（TypeError、Array、Object 等内置构造器）：
    // 其 `prototype` 等静态属性在后端 side table 中，V2 堆查不到。
    if value::is_native_callable(target) {
        if let Some(key) = &prop_str {
            return ctx.read_property_by_string_key(target, key);
        }
        return value::encode_undefined();
    }

    // 对象/数组/函数/闭包/bound → V2 堆路径
    if value::is_object(target)
        || value::is_array(target)
        || value::is_function(target)
        || value::is_closure(target)
        || value::is_bound(target)
    {
        let Some(handle) = ctx.handle_index_of(target) else {
            return value::encode_undefined();
        };
        let Some(name_id) = ctx.property_value_to_name_id(prop, true) else {
            return value::encode_undefined();
        };
        if let Some((slot_val, is_accessor, getter)) =
            ctx.get_property_slot_on_proto(handle, name_id)
        {
            if is_accessor {
                if value::is_undefined(getter) || value::is_null(getter) {
                    return value::encode_undefined();
                }
                return match ctx.call_js_async(getter, receiver, &[]).await {
                    Ok(v) => v,
                    Err(_) => value::encode_undefined(),
                };
            }
            return slot_val;
        }
        return value::encode_undefined();
    }

    // 兜底：用 read_property_by_string_key（覆盖 RegExp/prototype 等特殊路径）
    if let Some(key) = prop_str {
        return ctx.read_property_by_string_key(target, &key);
    }
    value::encode_undefined()
}

// ── Reflect.set (async, Proxy-aware) ──────────────────────────────────────

/// `Reflect.set(target, prop, val, receiver)` 异步路径（含 Proxy set trap）。
pub async fn reflect_set_impl_with_receiver<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    prop: Value,
    val: Value,
    receiver: Value,
) -> Value {
    if !value::is_js_object(target) && !value::is_array(target) && !value::is_function(target) {
        return value::encode_bool(false);
    }
    if value::is_proxy(target) {
        let (t, handler) = match proxy_target_handler(ctx, target, "set") {
            Ok(pair) => pair,
            Err(exc) => return exc,
        };
        if let Some(trap) = proxy_trap_handler_trap(ctx, handler, "set") {
            let result = match ctx
                .call_js_async(trap, handler, &[t, prop, val, receiver])
                .await
            {
                Ok(v) => v,
                Err(_) => return value::encode_bool(false),
            };
            return value::encode_bool(value::is_truthy(result));
        }
        // 无 trap → OrdinarySet on target
        let Some(name_id) = ctx.property_value_to_name_id(prop, true) else {
            return value::encode_bool(false);
        };
        return value::encode_bool(ordinary_set_by_name_id(ctx, t, receiver, name_id, val).await);
    }
    let Some(name_id) = ctx.property_value_to_name_id(prop, true) else {
        return value::encode_bool(false);
    };
    value::encode_bool(ordinary_set_by_name_id(ctx, target, receiver, name_id, val).await)
}

// ── OrdinarySet / DefineValueOnReceiver ───────────────────────────────────

/// §10.1.9: OrdinarySet with OwnDescriptor 搜索（V2 堆路径）。
pub async fn ordinary_set_by_name_id<E: ExecContext>(
    ctx: &mut E,
    obj: Value,
    receiver: Value,
    name_id: u32,
    val: Value,
) -> bool {
    let Some(handle) = ctx.handle_index_of(obj) else {
        return false;
    };
    // 沿原型链搜索属性槽
    let mut current = handle;
    let mut visited = std::collections::HashSet::new();
    loop {
        if !visited.insert(current) {
            return false;
        }
        if let Some((_slot_val, flags, _getter, setter)) = ctx.get_own_property_slot(current, name_id) {
            let is_accessor = (flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32) != 0;
            if is_accessor {
                // 调用 setter（非 getter）
                if !value::is_undefined(setter) && !value::is_null(setter) {
                    let _ = ctx.call_js_async(setter, receiver, &[val]).await;
                    return true;
                }
                return false;
            }
            if (flags & wjsm_ir::constants::FLAG_WRITABLE as u32) == 0 {
                return false;
            }
            // current == receiver → 直接写
            let current_val = ctx.encode_handle_as_value(current);
            if current_val == receiver {
                return ctx.set_property_by_name_id(current, name_id, val);
            }
            return define_value_on_receiver(ctx, receiver, name_id, val).await;
        }
        // 沿原型链上行
        let Some(proto) = ctx.prototype_of(current) else {
            return define_value_on_receiver(ctx, receiver, name_id, val).await;
        };
        if proto == 0xFFFF_FFFF || proto == current {
            return define_value_on_receiver(ctx, receiver, name_id, val).await;
        }
        current = proto;
    }
}

/// §10.1.9.2: define {value:V} on Receiver。
///
/// 先通过 `reflect_get_own_property_descriptor_on_object_async` 查询 receiver上
/// 是否已有该属性（对 proxy 会触发 getOwnPropertyDescriptor trap），再根据结果
/// 决定是更新还是新建（对 proxy 会触发 defineProperty trap）。
pub async fn define_value_on_receiver<E: ExecContext>(
    ctx: &mut E,
    receiver: Value,
    name_id: u32,
    val: Value,
) -> bool {
    if !value::is_object(receiver)
        && !value::is_function(receiver)
        && !value::is_array(receiver)
        && !value::is_proxy(receiver)
    {
        return false;
    }
    let prop = match ctx.name_id_to_property_key_value(name_id) {
        Some(v) => v,
        None => return false,
    };
    // 查询 receiver 上是否已有该属性（proxy 会触发 getOwnPropertyDescriptor trap）
    let existing_handle = Box::pin(
        reflect_get_own_property_descriptor_on_object_async(ctx, receiver, prop),
    )
    .await;
    if value::is_exception(existing_handle) {
        return false;
    }
    // 解析已有描述符
    if !value::is_undefined(existing_handle) && value::is_js_object(existing_handle) {
        if let Ok(desc) = parse_descriptor(ctx, existing_handle) {
            let completed = complete_property_descriptor(desc);
            if is_accessor_descriptor(&completed) {
                return false;
            }
            if completed.writable == Some(false) {
                return false;
            }
        }
    } else if !ctx.is_extensible(receiver) {
        return false;
    }
    // 执行 defineProperty（proxy 会触发 defineProperty trap）
    let desc_obj = crate::proxy_reflect::alloc_data_property_descriptor(ctx, val, true, true, true);
    if value::is_proxy(receiver) {
        let (t, handler) = match proxy_target_handler(ctx, receiver, "defineProperty") {
            Ok(pair) => pair,
            Err(_) => return false,
        };
        if let Some(trap) = proxy_trap_handler_trap(ctx, handler, "defineProperty") {
            let result = match ctx.call_js_async(trap, handler, &[t, prop, desc_obj]).await {
                Ok(v) => v,
                Err(_) => return false,
            };
            return value::is_truthy(result);
        }
        return ctx.define_property_or_throw(t, prop, desc_obj);
    }
    ctx.define_property_or_throw(receiver, prop, desc_obj)
}

// ── Reflect.apply / construct ─────────────────────────────────────────────

/// `Reflect.apply(target, thisArg, args)` 异步路径（含 Proxy apply trap）。
pub async fn reflect_apply_impl_async<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    this_arg: Value,
    args: &[Value],
) -> Value {
    match ctx.call_js_async(target, this_arg, args).await {
        Ok(v) => v,
        Err(_) => value::encode_undefined(),
    }
}

/// `Reflect.construct(target, args, newTarget)` 异步路径（含 Proxy construct trap）。
pub async fn reflect_construct_impl_async<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    args: &[Value],
    new_target: Value,
) -> Value {
    // Proxy construct trap
    if value::is_proxy(target) {
        let (t, handler) = match proxy_target_handler(ctx, target, "construct") {
            Ok(pair) => pair,
            Err(exc) => return exc,
        };
        if let Some(trap) = proxy_trap_handler_trap(ctx, handler, "construct") {
            // 构造 args 数组传给 trap
 let args_arr = ctx.alloc_array(args.len() as u32);
            for (i, arg) in args.iter().enumerate() {
                ctx.array_write_elem(args_arr, i as u32, *arg);
            }
            ctx.array_write_length(args_arr, args.len() as u32);
            let result = match ctx.call_js_async(trap, handler, &[t, args_arr, new_target]).await {
                Ok(v) => v,
                Err(_) => return ctx.make_type_error("Proxy construct trap failed"),
            };
            // §10.5.13 不变量：construct trap 必须返回对象
            if !value::is_js_object(result) {
                return ctx.make_type_error(
                    "TypeError: 'construct' on proxy: trap result is not an object",
                );
            }
            return result;
        }
        // 无 trap → 对 target 执行普通构造
        return ordinary_construct(ctx, t, args, new_target).await;
    }
    ordinary_construct(ctx, target, args, new_target).await
}

/// 普通构造：分配 this_obj，设置原型，调用 target。
async fn ordinary_construct<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    args: &[Value],
    new_target: Value,
) -> Value {
    let this_obj = ctx.alloc_object(4);
    // 通过 Reflect.get 读取 new_target.prototype（proxy-aware）
    let proto_prop = ctx.store_runtime_string(wjsm_host::RuntimeString::from_utf8_str("prototype"));
    let proto_val = Box::pin(reflect_get_impl_with_receiver_async(
        ctx,
        new_target,
        proto_prop,
        new_target,
    ))
    .await;
    if value::is_object(proto_val)
        || value::is_array(proto_val)
        || value::is_proxy(proto_val)
        || value::is_null(proto_val)
    {
        let proto_handle = ctx.value_to_proto_handle(proto_val);
        let _ = ctx.set_prototype_handle(this_obj, proto_handle);
    }
    let result = match ctx.call_js_async(target, this_obj, args).await {
        Ok(v) => v,
        Err(_) => return this_obj,
    };
    if value::is_js_object(result) {
        result
    } else {
        this_obj
    }
}

// ── Reflect.getPrototypeOf / setPrototypeOf ───────────────────────────────

/// `Reflect.getPrototypeOf(target)` 异步路径（含 Proxy getPrototypeOf trap）。
pub async fn reflect_get_prototype_of_async<E: ExecContext>(
    ctx: &mut E,
    target: Value,
) -> Value {
    if value::is_proxy(target) {
        let (t, handler) = match proxy_target_handler(ctx, target, "getPrototypeOf") {
            Ok(pair) => pair,
            Err(exc) => return exc,
        };
        if let Some(trap) = proxy_trap_handler_trap(ctx, handler, "getPrototypeOf") {
            let res = match ctx.call_js_async(trap, handler, &[t]).await {
                Ok(v) => v,
                Err(_) => value::encode_null(),
            };
            // 不变量检查: getPrototypeOf trap 返回值必须是 null 或对象
            if !value::is_null(res)
                && !value::is_object(res)
                && !value::is_array(res)
                && !value::is_proxy(res)
                && !value::is_function(res)
            {
                ctx.set_last_error(
                    "TypeError: Proxy getPrototypeOf must return an object or null".to_string(),
                );
                return value::encode_null();
            }
            // 不变量检查: 若 target 不可扩展，返回的原型必须与 target 原型一致
            if !ctx.is_extensible(t) {
                let target_proto = Box::pin(reflect_get_prototype_of_impl(ctx, t)).await;
                if res != target_proto {
                    ctx.set_last_error(
                        "TypeError: Proxy getPrototypeOf invariant violated: target is not extensible and trap returned different prototype"
                            .to_string(),
                    );
                    return value::encode_null();
                }
            }
            return res;
        }
        return Box::pin(reflect_get_prototype_of_impl(ctx, t)).await;
    }
    Box::pin(reflect_get_prototype_of_impl(ctx, target)).await
}

/// 非 Proxy 的 getPrototypeOf 实现（递归 Proxy 感知）。
pub async fn reflect_get_prototype_of_impl<E: ExecContext>(ctx: &mut E, target: Value) -> Value {
    if value::is_proxy(target) {
        return reflect_get_prototype_of_async(ctx, target).await;
    }
    // RegExp 原型
    if value::is_regexp(target) {
        return ctx.regexp_prototype();
    }
    let Some(handle) = ctx.handle_index_of(target) else {
        return value::encode_null();
    };
    match ctx.prototype_of(handle) {
        Some(proto) if proto != 0xFFFF_FFFF => ctx.encode_handle_as_value(proto),
        _ => value::encode_null(),
    }
}

/// `Reflect.setPrototypeOf(target, proto)` 异步路径（含 Proxy setPrototypeOf trap）。
pub async fn reflect_set_prototype_of_fn_impl<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    proto: Value,
) -> Value {
    if value::is_proxy(target) {
        let (t, handler) = match proxy_target_handler(ctx, target, "setPrototypeOf") {
            Ok(pair) => pair,
            Err(exc) => return exc,
        };
        if let Some(trap) = proxy_trap_handler_trap(ctx, handler, "setPrototypeOf") {
            let result = match ctx.call_js_async(trap, handler, &[t, proto]).await {
                Ok(v) => v,
                Err(_) => return value::encode_bool(false),
            };
            let trap_res = value::is_truthy(result);
            if trap_res && !ctx.is_extensible(t) {
                let current_proto = reflect_get_prototype_of_impl(ctx, t).await;
                if current_proto != proto {
                    ctx.set_last_error(
                        "TypeError: Proxy setPrototypeOf invariant violated: target is not extensible and new prototype is different"
                            .to_string(),
                    );
                    return value::encode_bool(false);
                }
            }
            return value::encode_bool(trap_res);
        }
        // 无 trap → 直接设 target 原型
        let proto_handle = ctx.value_to_proto_handle(proto);
        return value::encode_bool(ctx.set_prototype_handle(t, proto_handle));
    }
    let proto_handle = ctx.value_to_proto_handle(proto);
    value::encode_bool(ctx.set_prototype_handle(target, proto_handle))
}

// ── Proxy ownKeys trap ────────────────────────────────────────────────────

/// Proxy ownKeys 陷阱：返回陷阱结果数组，失败或应回退时返回 undefined。
pub async fn proxy_own_keys_trap_async<E: ExecContext>(ctx: &mut E, target: Value) -> Value {
    if !value::is_proxy(target) {
        return value::encode_undefined();
    }
    let (t, handler) = match proxy_target_handler(ctx, target, "ownKeys") {
        Ok(pair) => pair,
        Err(exc) => return exc,
    };
    let trap = ctx.read_data_property(handler, "ownKeys");
    if value::is_undefined(trap) || value::is_null(trap) {
        return reflect_own_keys_impl(ctx, t);
    }
    let keys_val = match ctx.call_js_async(trap, handler, &[t]).await {
        Ok(v) => v,
        Err(e) => {
            return ctx.make_type_error(&format!("Proxy ownKeys trap failed: {}", e));
        }
    };
    let keys = match extract_array_like_elements(ctx, keys_val).await {
        Ok(arr) => arr,
        Err(err) => return ctx.make_type_error(&err),
    };
    // 不变量检查：非可扩展 target 的 ownKeys 必须包含全部 target own keys
    let ext = ctx.is_extensible(t);
    let Some(target_handle) = ctx.handle_index_of(t) else {
        return value::encode_undefined();
    };
    let target_entries = ctx.own_property_entries(target_handle);
    let mut trap_keys_str = Vec::new();
    let mut trap_keys_sym = Vec::new();
    for key in &keys {
        if value::is_symbol(*key) {
            trap_keys_sym.push(*key);
        } else if let Ok(key_str) = ctx.value_to_key_string(*key) {
            trap_keys_str.push(key_str);
        }
    }
    let mut target_strings = Vec::new();
    let mut target_symbols = Vec::new();
    for (name_id, flags) in target_entries {
        let configurable = (flags & wjsm_ir::constants::FLAG_CONFIGURABLE as u32) != 0;
        // 判断 name_id 是 symbol 还是 string
        if let Some(key_val) = ctx.name_id_to_property_key_value(name_id) {
            if value::is_symbol(key_val) {
                target_symbols.push((key_val, configurable));
            } else if let Ok(key_str) = ctx.value_to_key_string(key_val) {
                target_strings.push((key_str, configurable));
            }
        }
    }
    let violates_invariant = if !ext {
        target_strings.len() != trap_keys_str.len()
            || target_symbols.len() != trap_keys_sym.len()
            || target_strings
                .iter()
                .any(|(key, _)| !trap_keys_str.contains(key))
            || target_symbols.iter().any(|(key, _)| {
                !trap_keys_sym.iter().any(|trap_key| {
                    crate::object_builtins::same_value_zero(ctx, *trap_key, *key)
                })
            })
    } else {
        target_strings
            .iter()
            .any(|(key, configurable)| !*configurable && !trap_keys_str.contains(key))
            || target_symbols.iter().any(|(key, configurable)| {
                !*configurable
                    && !trap_keys_sym
                        .iter()
                        .any(|trap_key| {
                            crate::object_builtins::same_value_zero(ctx, *trap_key, *key)
                        })
            })
    };
    if violates_invariant {
        return ctx.make_type_error("Proxy ownKeys invariant violated for V2 target");
    }
    let len = keys.len() as u32;
    let arr = ctx.alloc_array(len);
    for (i, key) in keys.into_iter().enumerate() {
        ctx.array_write_elem(arr, i as u32, key);
    }
    ctx.array_write_length(arr, len);
    arr
}

// ── Extract array-like ────────────────────────────────────────────────────

/// 从数组或类数组对象提取元素列表。
pub async fn extract_array_like_elements<E: ExecContext>(
    ctx: &mut E,
    arr_like: Value,
) -> Result<Vec<Value>, String> {
    let mut elements = Vec::new();
    if value::is_array(arr_like) {
        if ctx.handle_index_of(arr_like).is_none() {
            return Ok(elements);
        };
        let len = ctx.array_read_length(arr_like).unwrap_or(0);
        for index in 0..len {
            let elem = ctx.array_elem_at(arr_like, index).unwrap_or_else(value::encode_undefined);
            elements.push(elem);
        }
    } else if value::is_object(arr_like) || value::is_proxy(arr_like) {
        let len_prop = ctx.store_string_owned("length".to_string());
        let len_val = reflect_get_impl_with_receiver_async(ctx, arr_like, len_prop, arr_like).await;
        let len = if value::is_f64(len_val) {
            value::decode_f64(len_val) as usize
        } else {
            0
        };
        for i in 0..len {
            let idx_prop = value::encode_f64(i as f64);
            let val = reflect_get_impl_with_receiver_async(ctx, arr_like, idx_prop, arr_like).await;
            elements.push(val);
        }
    }
    Ok(elements)
}

// ── Object async 系列（Proxy 感知）────────────────────────────────────────

/// `Object.keys`：proxy 走 ownKeys 陷阱后按 enumerable 过滤字符串键。
pub async fn object_enumerable_own_keys_async<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    if !value::is_js_object(obj) {
        return ctx.alloc_array(0);
    }
    if value::is_proxy(obj) {
        let keys_arr = proxy_own_keys_trap_async(ctx, obj).await;
        if value::is_exception(keys_arr) {
            return keys_arr;
        }
        if value::is_undefined(keys_arr) {
            return ctx.alloc_array(0);
        }
        let keys = match extract_array_like_elements(ctx, keys_arr).await {
            Ok(k) => k,
            Err(_) => return ctx.alloc_array(0),
        };
        let mut out = Vec::new();
        for key in keys {
            if value::is_symbol(key) {
                continue;
            }
            // 通过 getOwnPropertyDescriptor 判断 enumerable
            let desc = reflect_get_own_property_descriptor_on_object_async(ctx, obj, key).await;
            if !value::is_undefined(desc) {
                let enumerable_val = ctx.read_data_property(desc, "enumerable");
                if value::is_truthy(enumerable_val) {
                    out.push(key);
                }
            }
        }
        let len = out.len() as u32;
        let arr = ctx.alloc_array(len);
        for (i, key) in out.into_iter().enumerate() {
            ctx.array_write_elem(arr, i as u32, key);
        }
        ctx.array_write_length(arr, len);
        return arr;
    }
    // 非 proxy：collect_own_property_names enumerable
    let names = ctx.collect_own_property_names(obj, true);
    let len = names.len() as u32;
    let arr = ctx.alloc_array(len);
    for (i, name) in names.into_iter().enumerate() {
        let key = ctx.store_string_owned(name);
        ctx.array_write_elem(arr, i as u32, key);
    }
    ctx.array_write_length(arr, len);
    arr
}

/// `Object.getOwnPropertyNames`：proxy 走 ownKeys 陷阱，仅保留字符串键。
pub async fn object_get_own_property_names_async<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    if !value::is_js_object(obj) {
        return ctx.alloc_array(0);
    }
    if value::is_proxy(obj) {
        let keys_arr = proxy_own_keys_trap_async(ctx, obj).await;
        if value::is_exception(keys_arr) {
            return keys_arr;
        }
        if value::is_undefined(keys_arr) {
            return ctx.alloc_array(0);
        }
        let keys = match extract_array_like_elements(ctx, keys_arr).await {
            Ok(k) => k,
            Err(_) => return ctx.alloc_array(0),
        };
        let out: Vec<Value> = keys.into_iter().filter(|k| !value::is_symbol(*k)).collect();
        let len = out.len() as u32;
        let arr = ctx.alloc_array(len);
        for (i, key) in out.into_iter().enumerate() {
            ctx.array_write_elem(arr, i as u32, key);
        }
        ctx.array_write_length(arr, len);
        return arr;
    }
    let names = ctx.collect_own_property_names(obj, false);
    let len = names.len() as u32;
    let arr = ctx.alloc_array(len);
    for (i, name) in names.into_iter().enumerate() {
        let key = ctx.store_string_owned(name);
        ctx.array_write_elem(arr, i as u32, key);
    }
    ctx.array_write_length(arr, len);
    arr
}

/// `Object.values`：enumerable 字符串键 + Reflect.get 取值。
pub async fn object_values_async<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    if !value::is_js_object(obj) {
        return ctx.alloc_array(0);
    }
    let keys_arr = object_enumerable_own_keys_async(ctx, obj).await;
    if value::is_exception(keys_arr) {
        return keys_arr;
    }
    let keys = match extract_array_like_elements(ctx, keys_arr).await {
        Ok(k) => k,
        Err(_) => return ctx.alloc_array(0),
    };
    let len = keys.len() as u32;
    let arr = ctx.alloc_array(len);
    for (i, key) in keys.iter().enumerate() {
        let val = reflect_get_impl_with_receiver_async(ctx, obj, *key, obj).await;
        ctx.array_write_elem(arr, i as u32, val);
    }
    ctx.array_write_length(arr, len);
    arr
}

/// `Object.entries`：enumerable 字符串键 + Reflect.get 取值。
pub async fn object_entries_async<E: ExecContext>(ctx: &mut E, obj: Value) -> Value {
    if !value::is_js_object(obj) {
        return ctx.alloc_array(0);
    }
    let keys_arr = object_enumerable_own_keys_async(ctx, obj).await;
    let keys = match extract_array_like_elements(ctx, keys_arr).await {
        Ok(k) => k,
        Err(_) => return ctx.alloc_array(0),
    };
    let len = keys.len() as u32;
    let arr = ctx.alloc_array(len);
    for (i, key) in keys.iter().enumerate() {
        let val = reflect_get_impl_with_receiver_async(ctx, obj, *key, obj).await;
        let pair = ctx.alloc_array(2);
        ctx.array_write_elem(pair, 0, *key);
        ctx.array_write_elem(pair, 1, val);
        ctx.array_write_elem(arr, i as u32, pair);
    }
    ctx.array_write_length(arr, len);
    arr
}

/// `Object.getOwnPropertySymbols`：proxy 走 ownKeys 陷阱，仅保留 Symbol 键。
pub async fn object_get_own_property_symbols_async<E: ExecContext>(
    ctx: &mut E,
    obj: Value,
) -> Value {
    if !value::is_js_object(obj) {
        return ctx.alloc_array(0);
    }
    if value::is_proxy(obj) {
        let keys_arr = proxy_own_keys_trap_async(ctx, obj).await;
        if value::is_exception(keys_arr) {
            return keys_arr;
        }
        if value::is_undefined(keys_arr) {
            return ctx.alloc_array(0);
        }
        let keys = match extract_array_like_elements(ctx, keys_arr).await {
            Ok(k) => k,
            Err(_) => return ctx.alloc_array(0),
        };
        let out: Vec<Value> = keys.into_iter().filter(|k| value::is_symbol(*k)).collect();
        let len = out.len() as u32;
        let arr = ctx.alloc_array(len);
        for (i, key) in out.into_iter().enumerate() {
            ctx.array_write_elem(arr, i as u32, key);
        }
        ctx.array_write_length(arr, len);
        return arr;
    }
    let symbols = ctx.collect_own_property_symbols(obj);
    let len = symbols.len() as u32;
    let arr = ctx.alloc_array(len);
    for (i, symbol) in symbols.into_iter().enumerate() {
        ctx.array_write_elem(arr, i as u32, symbol);
    }
    ctx.array_write_length(arr, len);
    arr
}

/// `Object.assign`：Set(target, key, value, true) for each enumerable own key。
pub async fn object_assign_impl_async<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    sources: &[Value],
) -> Value {
    if !value::is_object(target) && !value::is_function(target) && !value::is_array(target) {
        ctx.set_last_error("TypeError: target is not an object".to_string());
        return value::encode_undefined();
    }
    for &source_val in sources {
        if value::is_undefined(source_val) || value::is_null(source_val) {
            continue;
        }
        let source = if !value::is_js_object(source_val) {
            ctx.to_object(source_val)
        } else {
            source_val
        };
        let names = ctx.collect_own_property_names(source, true);
        for name in names {
            let name_val = ctx.store_string_owned(name.clone());
            let prop_val =
                reflect_get_impl_with_receiver_async(ctx, source, name_val, source).await;
            let Some(name_id) = ctx.property_value_to_name_id(name_val, true) else {
                continue;
            };
            if !ordinary_set_by_name_id(ctx, target, target, name_id, prop_val).await {
                return ctx.make_type_error("Cannot assign to read only property");
            }
        }
    }
    target
}

// ── Reflect.getOwnPropertyDescriptor (async, Proxy-aware) ─────────────────

/// JS 属性描述符（后端无关版本）。
#[derive(Debug, Clone)]
struct PropertyDescriptor {
    value: Option<Value>,
    writable: Option<bool>,
    enumerable: Option<bool>,
    configurable: Option<bool>,
    get: Option<Value>,
    set: Option<Value>,
}

fn is_accessor_descriptor(desc: &PropertyDescriptor) -> bool {
    desc.get.is_some() || desc.set.is_some()
}

fn is_data_descriptor(desc: &PropertyDescriptor) -> bool {
    desc.value.is_some() || desc.writable.is_some()
}

fn complete_property_descriptor(mut desc: PropertyDescriptor) -> PropertyDescriptor {
    if is_accessor_descriptor(&desc) {
        desc.get.get_or_insert_with(value::encode_undefined);
        desc.set.get_or_insert_with(value::encode_undefined);
    } else {
        desc.value.get_or_insert_with(value::encode_undefined);
        desc.writable.get_or_insert(false);
    }
    desc.enumerable.get_or_insert(false);
    desc.configurable.get_or_insert(false);
    desc
}

/// 解析 JS 对象形式的描述符为 PropertyDescriptor。
fn parse_descriptor<E: ExecContext>(ctx: &mut E, desc_obj: Value) -> Result<PropertyDescriptor, String> {
    if !value::is_object(desc_obj)
        && !value::is_function(desc_obj)
        && !value::is_array(desc_obj)
        && !value::is_proxy(desc_obj)
    {
        return Err("Invalid property descriptor".to_string());
    }
    let prop_value = ctx.read_data_property(desc_obj, "value");
    let prop_writable = ctx.read_data_property(desc_obj, "writable");
    let prop_enumerable = ctx.read_data_property(desc_obj, "enumerable");
    let prop_configurable = ctx.read_data_property(desc_obj, "configurable");
    let prop_get = ctx.read_data_property(desc_obj, "get");
    let prop_set = ctx.read_data_property(desc_obj, "set");

    // read_data_property 对不存在的属性返回 undefined；区分"显式 undefined"和"不存在"
    // 在不变量检查中，undefined 等同于不存在（规范 ToPropertyDescriptor 行为）
    let prop_value = if value::is_undefined(prop_value) { None } else { Some(prop_value) };
    let prop_writable = if value::is_undefined(prop_writable) { None } else { Some(prop_writable) };
    let prop_enumerable = if value::is_undefined(prop_enumerable) { None } else { Some(prop_enumerable) };
    let prop_configurable = if value::is_undefined(prop_configurable) { None } else { Some(prop_configurable) };
    let prop_get = if value::is_undefined(prop_get) { None } else { Some(prop_get) };
    let prop_set = if value::is_undefined(prop_set) { None } else { Some(prop_set) };

    if let Some(getter) = prop_get
        && !value::is_null(getter)
        && !ctx.is_callable(getter)
    {
        return Err("property getter must be callable".to_string());
    }
    if let Some(setter) = prop_set
        && !value::is_null(setter)
        && !ctx.is_callable(setter)
    {
        return Err("property setter must be callable".to_string());
    }

    let has_accessor = prop_get.is_some() || prop_set.is_some();
    if has_accessor && (prop_value.is_some() || prop_writable.is_some()) {
        return Err("Invalid property descriptor: cannot specify both accessor and value/writable".to_string());
    }

    Ok(PropertyDescriptor {
        value: prop_value,
        writable: prop_writable.map(|v| !value::is_falsy(v)),
        enumerable: prop_enumerable.map(|v| !value::is_falsy(v)),
        configurable: prop_configurable.map(|v| !value::is_falsy(v)),
        get: prop_get,
        set: prop_set,
    })
}

/// 从 target 的属性槽中提取 PropertyDescriptor。
fn get_target_descriptor<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    name_id: u32,
) -> Option<PropertyDescriptor> {
    let handle = ctx.handle_index_of(target)?;
    let (slot_val, flags, getter, setter) = ctx.get_own_property_slot(handle, name_id)?;
    let is_accessor = (flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32) != 0;
    Some(PropertyDescriptor {
        value: (!is_accessor).then_some(slot_val),
        writable: (!is_accessor).then_some((flags & wjsm_ir::constants::FLAG_WRITABLE as u32) != 0),
        enumerable: Some((flags & wjsm_ir::constants::FLAG_ENUMERABLE as u32) != 0),
        configurable: Some((flags & wjsm_ir::constants::FLAG_CONFIGURABLE as u32) != 0),
        get: is_accessor.then_some(getter),
        set: is_accessor.then_some(setter),
    })
}

fn is_compatible_property_descriptor<E: ExecContext>(
    ctx: &mut E,
    extensible: bool,
    desc: &PropertyDescriptor,
    current: Option<&PropertyDescriptor>,
) -> bool {
    let Some(current) = current else {
        return extensible;
    };
    let current_configurable = current.configurable.unwrap_or(false);
    if !current_configurable {
        if desc.configurable == Some(true) {
            return false;
        }
        if desc.enumerable != current.enumerable {
            return false;
        }
    }
    let current_is_data = is_data_descriptor(current);
    let desc_is_data = is_data_descriptor(desc);
    if current_is_data != desc_is_data {
        return current_configurable;
    }
    if current_is_data {
        if !current_configurable && current.writable == Some(false) {
            if desc.writable == Some(true) {
                return false;
            }
            let current_value = current.value.unwrap_or_else(value::encode_undefined);
            let desc_value = desc.value.unwrap_or_else(value::encode_undefined);
            if !crate::object_builtins::same_value_zero(ctx, current_value, desc_value) {
                return false;
            }
        }
        return true;
    }
    if !current_configurable {
        if desc.get != current.get || desc.set != current.set {
            return false;
        }
    }
    true
}

/// §10.5.11 [[GetOwnProperty]] 不变量验证。
fn validate_proxy_get_own_property_descriptor_result<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    name_id: Option<u32>,
    trap_result: Value,
) -> Result<(), String> {
    let target_desc = name_id.and_then(|id| get_target_descriptor(ctx, target, id));
    let extensible = ctx.is_extensible(target);

    if value::is_undefined(trap_result) {
        let Some(target_desc) = target_desc else {
            return Ok(());
        };
        if target_desc.configurable == Some(false) {
            return Err("TypeError: Proxy getOwnPropertyDescriptor invariant violated: non-configurable property must not be reported as undefined".to_string());
        }
        if !extensible {
            return Err("TypeError: Proxy getOwnPropertyDescriptor invariant violated: target is non-extensible and property cannot be reported as missing".to_string());
        }
        return Ok(());
    }

    if !value::is_js_object(trap_result) {
        return Err("TypeError: Proxy getOwnPropertyDescriptor trap must return an object or undefined".to_string());
    }

    let result_desc = complete_property_descriptor(parse_descriptor(ctx, trap_result)?);
    if !is_compatible_property_descriptor(ctx, extensible, &result_desc, target_desc.as_ref()) {
        return Err("TypeError: Proxy getOwnPropertyDescriptor invariant violated: descriptor is incompatible with target".to_string());
    }

    if result_desc.configurable == Some(false) {
        let Some(target_desc) = target_desc.as_ref() else {
            return Err("TypeError: Proxy getOwnPropertyDescriptor invariant violated: non-configurable descriptor is incompatible with target".to_string());
        };
        if target_desc.configurable != Some(false) {
            return Err("TypeError: Proxy getOwnPropertyDescriptor invariant violated: non-configurable descriptor is incompatible with target".to_string());
        }
        if is_data_descriptor(target_desc)
            && target_desc.writable == Some(false)
            && result_desc.writable == Some(true)
        {
            return Err("TypeError: Proxy getOwnPropertyDescriptor invariant violated: non-configurable descriptor is incompatible with target".to_string());
        }
    }

    Ok(())
}

/// 通过 Reflect.getOwnPropertyDescriptor（含 proxy 陷阱 + 不变量验证）获取描述符。
pub async fn reflect_get_own_property_descriptor_on_object_async<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    prop: Value,
) -> Value {
    if value::is_proxy(target) {
        let (t, handler) = match proxy_target_handler(ctx, target, "getOwnPropertyDescriptor") {
            Ok(pair) => pair,
            Err(exc) => return exc,
        };
        let trap = ctx.read_data_property(handler, "getOwnPropertyDescriptor");
        if !value::is_undefined(trap) && !value::is_null(trap) {
            let descriptor = match ctx.call_js_async(trap, handler, &[t, prop]).await {
                Ok(d) => d,
                Err(e) => {
                    ctx.set_last_error(format!(
                        "TypeError: getOwnPropertyDescriptor trap failed: {e}"
                    ));
                    return value::encode_undefined();
                }
            };
            // §10.5.11 不变量验证
            let name_id = ctx.property_value_to_name_id(prop, false);
            if let Err(error) =
                validate_proxy_get_own_property_descriptor_result(ctx, t, name_id, descriptor)
            {
                ctx.set_last_error(error);
                return value::encode_undefined();
            }
            return descriptor;
        }
        return reflect_get_own_property_descriptor_impl(ctx, t, prop);
    }
    reflect_get_own_property_descriptor_impl(ctx, target, prop)
}

// ── Reentrant proxy async traps（$obj_get / $obj_set / $obj_delete）──────

/// `proxy_trap_get`：Proxy [[Get]] 内部方法。
pub async fn proxy_trap_internal_get_async<E: ExecContext>(
    ctx: &mut E,
    proxy: Value,
    name_id: i32,
) -> Value {
    let (target, handler) = match proxy_trap_proxy_entry(ctx, proxy, "get") {
        Ok(pair) => pair,
        Err(exc) => return exc,
    };
    if let Some(trap) = proxy_trap_handler_trap(ctx, handler, "get") {
        let prop = proxy_trap_property_key_value(ctx, name_id);
        return match ctx
            .call_js_async(trap, handler, &[target, prop, proxy])
            .await
        {
            Ok(v) => v,
            Err(_) => value::encode_undefined(),
        };
    }
    // 无 trap → target OrdinaryGet（异步，避免 block_in_place）
    let prop = proxy_trap_property_key_value(ctx, name_id);
    ordinary_get_async(ctx, target, prop, proxy).await
}

/// `proxy_trap_set`：Proxy [[Set]] 内部方法（返回 void，不可捕获异常）。
pub async fn proxy_trap_internal_set_async<E: ExecContext>(
    ctx: &mut E,
    proxy: Value,
    name_id: i32,
    val: Value,
) {
    let (target, handler) = match proxy_trap_proxy_entry(ctx, proxy, "set") {
        Ok(pair) => pair,
        Err(_) => {
            ctx.set_last_error(
                "TypeError: Cannot perform 'set' on a proxy that has been revoked".to_string(),
            );
            return;
        }
    };
    if let Some(trap) = proxy_trap_handler_trap(ctx, handler, "set") {
        let prop = proxy_trap_property_key_value(ctx, name_id);
        let _ = ctx.call_js_async(trap, handler, &[target, prop, val, proxy]).await;
        return;
    }
    // 无 trap → target OrdinarySet
    let prop = proxy_trap_property_key_value(ctx, name_id);
    let Some(nid) = ctx.property_value_to_name_id(prop, true) else {
        return;
    };
    let _ = ordinary_set_by_name_id(ctx, target, proxy, nid, val).await;
}

/// `proxy_trap_delete`：Proxy [[Delete]] 内部方法。
pub async fn proxy_trap_internal_delete_async<E: ExecContext>(
    ctx: &mut E,
    proxy: Value,
    name_id: i32,
) -> Value {
    let (target, handler) = match proxy_trap_proxy_entry(ctx, proxy, "deleteProperty") {
        Ok(pair) => pair,
        Err(exc) => return exc,
    };
    if let Some(trap) = proxy_trap_handler_trap(ctx, handler, "deleteProperty") {
        let prop = proxy_trap_property_key_value(ctx, name_id);
        let result = match ctx.call_js_async(trap, handler, &[target, prop]).await {
            Ok(v) => v,
            Err(_) => return value::encode_bool(false),
        };
        return value::encode_bool(value::is_truthy(result));
    }
    value::encode_bool(true)
}
