use wjsm_ir::{Builtin, constants, value};
use wjsm_native_abi::NativeVmContext;

use super::fail_dispatch;
use crate::{NativeAgentState, NativeCallableKind, PropertyKey};
use wjsm_host::RuntimeString;

const DATA_FLAGS: u32 =
    (constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE) as u32;
const HIDDEN_DATA_FLAGS: u32 = (constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE) as u32;

/// mapped arguments 对象的 [[ParameterMap]] 锚点。
///
/// 规范把 parameter map 描述成一个「属性为访问器、闭包到形参绑定」的对象；这里
/// 等价地记下形参所在的共享 env 对象与每个下标对应的 env 属性键，访问器对
/// （`NativeCallableKind::ArgumentsMapGetter` / `ArgumentsMapSetter`）按下标回查。
#[derive(Debug)]
pub(crate) struct ArgumentsParamMap {
    /// 形参所在的共享 env 对象（编码值）。
    pub(crate) env: i64,
    /// 按形参次序排列的 env 属性键；`None` 是同名形参里靠前的那些下标，按
    /// §10.4.4 步骤 21 的 mappedNames 去重规则不进 parameter map。
    pub(crate) keys: Vec<Option<PropertyKey>>,
}

pub(super) fn dispatch_arguments(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    let mapped = match builtin {
        Builtin::CreateMappedArgumentsObject => true,
        Builtin::CreateUnmappedArgumentsObject => false,
        Builtin::BindArgumentsParamMap => return Some(bind_param_map(ctx, state, args)),
        _ => return None,
    };
    Some(create(ctx, state, mapped, args))
}

/// `Builtin::BindArgumentsParamMap`：为落在实参个数内的形参下标装上映射访问器。
///
/// 规范只映射 `min(形参个数, 实参个数)` 个下标（§10.2.11 步骤 22 → §10.4.4
/// CreateMappedArgumentsObject 步骤 20 的 `index < numberOfParameters` 与
/// `index < len` 双重界）：没有实际传入的形参不进 parameter map，写形参不会
/// 让 `arguments` 长出新下标。
fn bind_param_map(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(([arguments, env], keys)) = args.split_first_chunk::<2>() else {
        return fail_dispatch(ctx);
    };
    let (arguments, env) = (*arguments, *env);
    if !value::is_js_object(arguments) || !value::is_js_object(env) {
        return fail_dispatch(ctx);
    }
    let handle = value::decode_handle(arguments);
    if state
        .gc
        .heap()
        .object_type(handle)
        .is_ok_and(|kind| kind != u32::from(wjsm_ir::HEAP_TYPE_ARGUMENTS))
    {
        return fail_dispatch(ctx);
    }
    let Ok(length) = arguments_length(state, handle) else {
        return fail_dispatch(ctx);
    };
    let mapped_count = keys.len().min(length);
    let mut resolved = Vec::with_capacity(mapped_count);
    for key in &keys[..mapped_count] {
        if value::is_undefined(*key) {
            resolved.push(None);
            continue;
        }
        let Some(key) = super::runtime::property_key(state, *key) else {
            return fail_dispatch(ctx);
        };
        resolved.push(Some(key));
    }
    if resolved.iter().all(Option::is_none) {
        return value::encode_undefined();
    }
    state
        .arguments_param_maps
        .insert(handle, ArgumentsParamMap { env, keys: resolved });
    for index in 0..mapped_count {
        let skip = state
            .arguments_param_maps
            .get(&handle)
            .is_none_or(|map| map.keys.get(index).is_none_or(Option::is_none));
        if skip {
            continue;
        }
        if !install_mapped_accessor(state, handle, index) {
            state.arguments_param_maps.remove(&handle);
            return fail_dispatch(ctx);
        }
    }
    value::encode_undefined()
}

/// 把下标 `index` 的数据属性换成映射访问器，保留既有的 enumerable/configurable。
fn install_mapped_accessor(state: &mut NativeAgentState, handle: u32, index: usize) -> bool {
    let Ok(index) = u32::try_from(index) else {
        return false;
    };
    let Some(key) = state.intern_property_string(RuntimeString::from(index.to_string())) else {
        return false;
    };
    let flags = match state.gc.heap().get_property_slot(handle, key) {
        Ok(Some(property)) => property.flags & !(constants::FLAG_WRITABLE as u32),
        _ => return false,
    };
    let Some(getter) = state.native_callable(NativeCallableKind::ArgumentsMapGetter(handle, index))
    else {
        return false;
    };
    let Some(setter) = state.native_callable(NativeCallableKind::ArgumentsMapSetter(handle, index))
    else {
        return false;
    };
    state
        .gc
        .heap()
        .define_accessor_property_with_flags(handle, key, getter as u64, setter as u64, flags)
        .is_ok()
}

/// arguments 对象的 `length` 自有属性（创建时写死为实参个数）。
fn arguments_length(state: &mut NativeAgentState, handle: u32) -> Result<usize, ()> {
    let key = state.intern_property_string("length".into()).ok_or(())?;
    match state.gc.heap().get_property_slot(handle, key) {
        Ok(Some(property)) => {
            let stored = property.value as i64;
            if !value::is_f64(stored) {
                return Err(());
            }
            let length = value::decode_f64(stored);
            if !(0.0..=f64::from(u32::MAX)).contains(&length) {
                return Err(());
            }
            Ok(length as usize)
        }
        _ => Err(()),
    }
}

/// `(env, key)`：下标 `index` 当前映射到的形参绑定，未映射时为 `None`。
fn mapped_binding(
    state: &NativeAgentState,
    handle: u32,
    index: u32,
) -> Option<(i64, PropertyKey)> {
    let map = state.arguments_param_maps.get(&handle)?;
    let key = map.keys.get(index as usize).copied().flatten()?;
    Some((map.env, key))
}

/// MakeArgGetter（ES §10.4.4.7）：读形参绑定的当前值。
pub(crate) fn map_getter(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    index: u32,
) -> i64 {
    let Some((env, key)) = mapped_binding(state, handle, index) else {
        return value::encode_undefined();
    };
    super::runtime::get_property(ctx, state, env, super::runtime::encoded_property_key(key))
        .unwrap_or_else(|()| fail_dispatch(ctx))
}

/// MakeArgSetter（ES §10.4.4.8）：写形参绑定。
pub(crate) fn map_setter(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    index: u32,
    stored: i64,
) -> i64 {
    let Some((env, key)) = mapped_binding(state, handle, index) else {
        return value::encode_undefined();
    };
    match super::runtime::ordinary_set_key(ctx, state, env, key, stored, env) {
        Ok(_) => value::encode_undefined(),
        Err(exception) => exception,
    }
}

/// 把所有仍在映射中的下标固化成数据属性并丢弃 [[ParameterMap]]。
///
/// `Object.freeze` 走的是 SetIntegrityLevel 的
/// `DefinePropertyOrThrow(O, k, {[[Writable]]: false, [[Configurable]]: false})`，
/// 对 mapped 下标即 §10.4.4.2 步骤 7.b.ii 的断映射。宿主的 freeze 实现按标志位
/// 批量收紧，看不到「访问器对其实代表数据属性」这层，所以在收紧前先在这里断开。
pub(crate) fn unmap_all(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
) -> Result<(), i64> {
    let Some(count) = state
        .arguments_param_maps
        .get(&handle)
        .map(|map| map.keys.len())
    else {
        return Ok(());
    };
    for index in 0..count {
        let Ok(index) = u32::try_from(index) else {
            return Err(fail_dispatch(ctx));
        };
        let Some(key) = state.intern_property_string(RuntimeString::from(index.to_string())) else {
            return Err(fail_dispatch(ctx));
        };
        if mapped_index(state, handle, key) != Some(index) {
            continue;
        }
        let value = map_getter(ctx, state, handle, index);
        if value::is_exception(value) {
            return Err(value);
        }
        let flags = match state.gc.heap().get_property_slot(handle, key) {
            Ok(Some(property)) => {
                (property.flags & !(constants::FLAG_IS_ACCESSOR as u32))
                    | constants::FLAG_WRITABLE as u32
            }
            _ => return Err(fail_dispatch(ctx)),
        };
        if state
            .gc
            .heap()
            .define_data_property(handle, key, value as u64, flags)
            .is_err()
        {
            return Err(fail_dispatch(ctx));
        }
    }
    state.arguments_param_maps.remove(&handle);
    Ok(())
}

/// 下标 `key` 当前是否仍在 [[ParameterMap]] 里。
///
/// 判据就是自有属性本身：只有映射访问器对才算已映射。`delete arguments[i]`、
/// 用户 defineProperty 成访问器、或 §10.4.4.2 步骤 7.b.ii 断映射后改回数据属性，
/// 都会让这里自然返回 `None`，无需另设「已断开」标志位。
pub(crate) fn mapped_index(
    state: &NativeAgentState,
    handle: u32,
    key: PropertyKey,
) -> Option<u32> {
    if !state.arguments_param_maps.contains_key(&handle) {
        return None;
    }
    let property = state.gc.heap().get_property_slot(handle, key).ok()??;
    if property.flags & (constants::FLAG_IS_ACCESSOR as u32) == 0 {
        return None;
    }
    match state.native_callable_kind(property.getter as i64)? {
        NativeCallableKind::ArgumentsMapGetter(owner, index) if owner == handle => Some(index),
        _ => None,
    }
}

pub(super) fn create(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    mapped: bool,
    args: &[i64],
) -> i64 {
    let Some(source) = args
        .first()
        .copied()
        .filter(|source| value::is_array(*source))
    else {
        return fail_dispatch(ctx);
    };
    let source_handle = value::decode_handle(source);
    let Ok(length) = state.gc.heap().array_length(source_handle) else {
        return fail_dispatch(ctx);
    };
    // 固定布局一次分配到位：索引属性 length 个 + "length" + @@iterator +
    // callee（mapped 为数据属性占 1 槽；unmapped 为 accessor 占 getter/setter
    // 2 槽）。容量不足会触发 shape 扩容 relocate，而本对象可能仍在 native
    // TLAB 中未物化，扩容将以 NativeTlabNeedsMaterialization 失败。
    //
    // mapped 还要为 [[ParameterMap]] 预留：随后的 BindArgumentsParamMap 会把前
    // `min(形参个数, 实参个数)` 个下标换成访问器对，每个多占 1 槽。
    let extra_slots = if mapped { 3 } else { 4 };
    let mapped_slots = if mapped {
        args.get(1)
            .copied()
            .filter(|count| value::is_f64(*count))
            .map(value::decode_f64)
            .filter(|count| (0.0..=f64::from(u32::MAX)).contains(count))
            .map_or(length, |count| length.min(count as u32))
    } else {
        0
    };
    let capacity = length
        .saturating_add(mapped_slots)
        .saturating_add(extra_slots);
    let Ok(arguments) = state.allocate_object_with_gc_retry(ctx, capacity, false) else {
        return fail_dispatch(ctx);
    };
    let handle = value::decode_handle(arguments);
    if state
        .gc
        .heap()
        .set_object_type(handle, wjsm_ir::HEAP_TYPE_ARGUMENTS)
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    for index in 0..length {
        let stored = state
            .gc
            .heap()
            .get_element(source_handle, index)
            .ok()
            .flatten()
            .map(|stored| stored as i64)
            .unwrap_or_else(value::encode_undefined);
        let Some(key) = state.intern_property_string(RuntimeString::from(index.to_string())) else {
            return fail_dispatch(ctx);
        };
        if state
            .gc
            .heap()
            .define_data_property(handle, key, stored as u64, DATA_FLAGS)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    if !define_named(
        state,
        handle,
        "length",
        value::encode_f64(f64::from(length)),
        HIDDEN_DATA_FLAGS,
    ) {
        return fail_dispatch(ctx);
    }
    let Some(iterator) =
        state.native_callable(NativeCallableKind::Builtin(Builtin::IteratorFrom, true))
    else {
        return fail_dispatch(ctx);
    };
    let iterator_key = PropertyKey::symbol(wjsm_ir::wk_symbol::ITERATOR);

    if state
        .gc
        .heap()
        .define_data_property(handle, iterator_key, iterator as u64, HIDDEN_DATA_FLAGS)
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    if mapped {
        if let Some(callee) = args
            .get(2)
            .copied()
            .filter(|callee| !value::is_undefined(*callee))
            && !define_named(state, handle, "callee", callee, HIDDEN_DATA_FLAGS)
        {
            return fail_dispatch(ctx);
        }
    } else {
        let Some(thrower) = state.native_callable(NativeCallableKind::ArgumentsStrictCallee) else {
            return fail_dispatch(ctx);
        };
        let Some(key) = state.intern_property_string("callee".into()) else {
            return fail_dispatch(ctx);
        };
        if state
            .gc
            .heap()
            .define_accessor_property_with_flags(handle, key, thrower as u64, thrower as u64, 0)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    arguments
}

pub(crate) fn strict_callee_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    super::modules::named_error_object(
        state,
        "TypeError",
        "'callee' and 'caller' properties are not defined".into(),
    )
    .and_then(|error| state.create_exception(error))
    .unwrap_or_else(|| fail_dispatch(ctx))
}

fn define_named(
    state: &mut NativeAgentState,
    object: u32,
    name: &str,
    stored: i64,
    flags: u32,
) -> bool {
    let Some(key) = state.intern_property_string(name.into()) else {
        return false;
    };
    state
        .gc
        .heap()
        .define_data_property(object, key, stored as u64, flags)
        .is_ok()
}
