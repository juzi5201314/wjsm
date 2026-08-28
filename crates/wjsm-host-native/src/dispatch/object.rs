use wjsm_gc::PROTO_NULL_SENTINEL;
use wjsm_ir::{Builtin, HEAP_TYPE_ARGUMENTS, constants, value, wk_symbol};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{
    fail_dispatch, get_property, iterator_done, iterator_from, iterator_value, object_handle,
    ordinary_set, property_key, strict_equal, type_error,
};
use crate::{NativeAgentState, NativeCallableKind, PropertyKey};

const ENUMERABLE: u32 = constants::FLAG_ENUMERABLE as u32;
const CONFIGURABLE: u32 = constants::FLAG_CONFIGURABLE as u32;
const WRITABLE: u32 = constants::FLAG_WRITABLE as u32;
const ACCESSOR: u32 = constants::FLAG_IS_ACCESSOR as u32;

pub(super) fn dispatch_object(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::ObjectKeys => enumerate(ctx, state, args, EnumerationKind::Keys),
        Builtin::ObjectValues => enumerate(ctx, state, args, EnumerationKind::Values),
        Builtin::ObjectEntries => enumerate(ctx, state, args, EnumerationKind::Entries),
        Builtin::ObjectGetOwnPropertyNames => enumerate(ctx, state, args, EnumerationKind::Names),
        Builtin::ObjectGetOwnPropertySymbols => {
            let Some(object) = args.first().copied() else {
                return Some(fail_dispatch(ctx));
            };
            let symbols: Vec<_> = if value::is_proxy(object) {
                match super::proxy::own_keys(ctx, state, object) {
                    Ok(keys) => keys
                        .into_iter()
                        .filter(|key| value::is_symbol(*key))
                        .collect(),
                    Err(exception) => return Some(exception),
                }
            } else {
                let Some(properties) = own_keys(state, object, false) else {
                    return Some(fail_dispatch(ctx));
                };
                properties
                    .into_iter()
                    .filter_map(|(key, _)| value::is_symbol(key).then_some(key))
                    .collect()
            };
            state
                .allocate_array_values_with_gc_retry(ctx, &symbols)
                .unwrap_or_else(|_| fail_dispatch(ctx))
        }
        Builtin::ObjectRest => object_rest(ctx, state, args),
        Builtin::ObjectAssign => assign(ctx, state, args),
        Builtin::ObjectCreate => create(ctx, state, args),
        Builtin::ObjectGetPrototypeOf => get_prototype(ctx, state, args),
        Builtin::ObjectSetPrototypeOf => set_prototype(ctx, state, args),
        Builtin::ObjectIs => object_is(ctx, state, args),
        Builtin::GetOwnPropDesc => get_own_property_descriptor(ctx, state, args),
        Builtin::DefineProperty => define_property(ctx, state, args),
        Builtin::ObjectGetOwnPropertyDescriptors => get_own_property_descriptors(ctx, state, args),
        Builtin::ObjectDefineProperties => define_properties(ctx, state, args),
        Builtin::ObjectPreventExtensions => prevent_extensions(ctx, state, args),
        Builtin::ObjectIsExtensible => is_extensible(ctx, state, args),
        Builtin::ObjectSeal => seal_or_freeze(ctx, state, args, false),
        Builtin::ObjectFreeze => seal_or_freeze(ctx, state, args, true),
        Builtin::ObjectIsSealed => is_sealed_or_frozen(ctx, state, args, false),
        Builtin::ObjectFromEntries => from_entries(ctx, state, args),
        Builtin::ObjectGroupBy => group_by(ctx, state, args),
        Builtin::ObjectIsFrozen => is_sealed_or_frozen(ctx, state, args, true),
        Builtin::ObjectProtoToString => object_proto_to_string(ctx, state, args),
        Builtin::ObjectProtoValueOf => object_proto_value_of(ctx, state, args),
        Builtin::CreateGlobalObject => create_global_object(ctx, state),
        _ => return None,
    })
}

/// `Object.prototype.toString`：解包 proxy 后按类型/内置 tag 生成 `[object Tag]`，
/// 尊重 `Symbol.toStringTag` 自定义 tag。
fn object_proto_to_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(input) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let tag_input = if value::is_proxy(input) {
        state
            .proxies
            .get(usize::try_from(value::decode_proxy_handle(input)).unwrap_or(usize::MAX))
            .and_then(|proxy| proxy.as_ref())
            .map_or(input, |proxy| proxy.target)
    } else {
        input
    };
    let default_tag = if value::is_undefined(tag_input) {
        "Undefined"
    } else if value::is_null(tag_input) {
        "Null"
    } else if value::is_bool(tag_input) {
        "Boolean"
    } else if value::is_f64(tag_input) {
        "Number"
    } else if value::is_string(tag_input) {
        "String"
    } else if value::is_bigint(tag_input) {
        "BigInt"
    } else if value::is_symbol(tag_input) {
        "Symbol"
    } else if value::is_array(tag_input) {
        "Array"
    } else if value::is_callable(tag_input) {
        "Function"
    } else if value::is_regexp(tag_input) {
        "RegExp"
    } else if value::is_js_object(tag_input)
        && state
            .gc
            .heap()
            .object_type(value::decode_handle(tag_input))
            .is_ok_and(|kind| kind == u32::from(HEAP_TYPE_ARGUMENTS))
    {
        "Arguments"
    } else if value::is_js_object(tag_input)
        && state
            .error_objects
            .contains(&value::decode_handle(tag_input))
    {
        "Error"
    } else if let Some(primitive) = value::is_js_object(tag_input)
        .then(|| value::decode_handle(tag_input))
        .and_then(|handle| state.boxed_primitives.get(&handle))
    {
        if value::is_bool(*primitive) {
            "Boolean"
        } else if value::is_f64(*primitive) {
            "Number"
        } else if value::is_string(*primitive) {
            "String"
        } else if value::is_bigint(*primitive) {
            "BigInt"
        } else if value::is_symbol(*primitive) {
            "Symbol"
        } else {
            "Object"
        }
    } else {
        "Object"
    };
    let tag = if value::is_null(input) || value::is_undefined(input) {
        default_tag.to_owned()
    } else {
        let key = value::encode_handle(value::TAG_SYMBOL, wk_symbol::TO_STRING_TAG);
        let custom = get_property(ctx, state, input, key).unwrap_or_else(|()| fail_dispatch(ctx));
        if value::is_exception(custom) {
            return custom;
        }
        state
            .string_owned(custom)
            .and_then(|text| text.to_utf8())
            .unwrap_or_else(|| default_tag.to_owned())
    };
    state
        .intern_text(format!("[object {tag}]"), value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

/// `Object.prototype.valueOf`：对象自身直接返回。
fn object_proto_value_of(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let _ = state;
    let _ = ctx;
    args.first().copied().unwrap_or_else(|| fail_dispatch(ctx))
}

/// 创建（或复用缓存的）全局对象，惰性初始化内置原型。
fn create_global_object(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    if let Some(global) = state.global_object {
        global
    } else if state.ensure_intrinsic_prototypes().is_err() {
        fail_dispatch(ctx)
    } else {
        match state.allocate_object_with_gc_retry(ctx, 0, false) {
            Ok(global) => {
                state.global_object = Some(global);
                global
            }
            Err(_) => fail_dispatch(ctx),
        }
    }
}

pub(crate) fn construct_object(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if value::is_js_object(input) || value::is_regexp(input) {
        return input;
    }
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 0, false) else {
        return fail_dispatch(ctx);
    };
    if !value::is_null(input) && !value::is_undefined(input) {
        state
            .boxed_primitives
            .insert(value::decode_handle(object), input);
    }
    object
}

#[derive(Clone, Copy)]
enum EnumerationKind {
    Entries,
    Keys,
    Names,
    Values,
}

/// 可调用对象的自有键：先物化 `length`/`name`，顺序与 CreateBuiltinFunction 一致。
fn callable_own_keys(
    state: &mut NativeAgentState,
    encoded: i64,
    enumerable_only: bool,
) -> Option<Vec<(i64, i64)>> {
    let callable = value::strip_gc_color(encoded);
    let length_key = state.intern_property_string("length".into())?;
    let name_key = state.intern_property_string("name".into())?;
    let _ = state.callable_property(callable, length_key);
    let _ = state.callable_property(callable, name_key);
    let mut properties = Vec::new();
    for key in [length_key, name_key] {
        push_callable_own(state, callable, key, enumerable_only, &mut properties)?;
    }

    let extras: Vec<PropertyKey> = state
        .callable_properties
        .keys()
        .chain(state.callable_accessors.keys())
        .filter_map(|(owner, key)| {
            (*owner == callable && *key != length_key && *key != name_key).then_some(*key)
        })
        .collect();
    let mut extras = extras;
    extras.sort_unstable();
    extras.dedup();
    for key in extras {
        push_callable_own(state, callable, key, enumerable_only, &mut properties)?;
    }
    Some(properties)
}

fn push_callable_own(
    state: &mut NativeAgentState,
    callable: i64,
    key: PropertyKey,
    enumerable_only: bool,
    properties: &mut Vec<(i64, i64)>,
) -> Option<()> {
    let flags = state
        .callable_property_flags
        .get(&(callable, key))
        .copied()
        .unwrap_or(0);
    if enumerable_only && flags & ENUMERABLE == 0 {
        return Some(());
    }
    let stored = if let Some((getter, _)) = state.callable_accessors.get(&(callable, key)).copied()
    {
        getter
    } else {
        state.callable_properties.get(&(callable, key)).copied()?
    };
    properties.push((super::runtime::encoded_property_key(key), stored));
    Some(())
}

/// PropertyKey → canonical array index（ECMA array index 字符串判定）。
fn canonical_array_index_key(state: &NativeAgentState, key: PropertyKey) -> Option<u32> {
    if key.is_symbol() {
        return None;
    }
    super::runtime::array_index(state, key.to_value())
}

pub(crate) fn own_keys(
    state: &mut NativeAgentState,
    encoded: i64,
    enumerable_only: bool,
) -> Option<Vec<(i64, i64)>> {
    if value::is_callable(encoded) {
        return callable_own_keys(state, encoded, enumerable_only);
    }
    if value::is_string(encoded) {
        let len = state.string_owned(encoded)?.utf16_len();
        let mut properties = Vec::with_capacity(len);
        for index in 0..len {
            let unit = state.string_owned(encoded)?.code_unit_at(index)?;
            let key = state.intern_text(index.to_string(), value::TAG_STRING)?;
            let stored = state.intern_runtime_string(
                wjsm_host::RuntimeString::from_utf16_units(vec![unit]),
                value::TAG_STRING,
            )?;
            properties.push((key, stored));
        }
        return Some(properties);
    }
    let handle = object_handle(encoded)?;
    if super::async_generator::is_async_generator(state, encoded) {
        let mut properties = Vec::with_capacity(4);
        for (name, builtin) in [
            ("next", Builtin::AsyncGeneratorNext),
            ("return", Builtin::AsyncGeneratorReturn),
            ("throw", Builtin::AsyncGeneratorThrow),
        ] {
            let key = state.intern_text(name.into(), value::TAG_STRING)?;
            let callable = state.native_callable(NativeCallableKind::Builtin(builtin, true))?;
            properties.push((key, callable));
        }
        if !enumerable_only {
            let key = value::encode_handle(value::TAG_SYMBOL, wjsm_ir::wk_symbol::ASYNC_ITERATOR);
            let callable = state.native_callable(NativeCallableKind::Builtin(
                Builtin::ObjectProtoValueOf,
                true,
            ))?;
            properties.push((key, callable));
        }
        return Some(properties);
    }
    if let Some(callable) = super::streams::async_iterator_property(state, encoded) {
        if enumerable_only {
            return Some(Vec::new());
        }
        let key = value::encode_handle(value::TAG_SYMBOL, wjsm_ir::wk_symbol::ASYNC_ITERATOR);
        let callable = state.native_callable(NativeCallableKind::Stream(callable))?;
        return Some(vec![(key, callable)]);
    }
    if value::is_array(encoded) {
        let length = state.gc.heap().array_length(handle).ok()?;
        let named = state
            .array_property_order
            .get(&handle)
            .cloned()
            .unwrap_or_default();
        let mut properties =
            Vec::with_capacity(length as usize + named.len() + usize::from(!enumerable_only));
        for index in 0..length {
            let element = state.gc.heap().get_element(handle, index).ok().flatten()? as i64;
            if value::is_array_hole(element) {
                continue;
            }
            let key = state.intern_text(index.to_string(), value::TAG_STRING)?;
            properties.push((key, element));
        }
        if !enumerable_only {
            let key = state.intern_text("length".into(), value::TAG_STRING)?;
            properties.push((key, value::encode_f64(f64::from(length))));
        }
        for symbols in [false, true] {
            for key in named.iter().copied() {
                if key.is_symbol() != symbols {
                    continue;
                }
                let flags = state
                    .array_property_flags
                    .get(&(handle, key))
                    .copied()
                    .or_else(|| {
                        state
                            .array_accessors
                            .get(&(handle, key))
                            .map(|(_, _, flags)| *flags)
                    })?;
                if enumerable_only && flags & ENUMERABLE == 0 {
                    continue;
                }
                let stored = state
                    .array_properties
                    .get(&(handle, key))
                    .copied()
                    .unwrap_or_else(value::encode_undefined);
                properties.push((super::runtime::encoded_property_key(key), stored));
            }
        }
        return Some(properties);
    }
    let slots = state.gc.heap().own_property_slots(handle).ok()?;
    let mut index_keys = Vec::new();
    let mut string_keys = Vec::new();
    let mut symbol_keys = Vec::new();
    for (key, flags) in slots {
        if enumerable_only && flags & ENUMERABLE == 0 {
            continue;
        }
        if key.is_symbol() {
            symbol_keys.push((key, flags));
        } else if let Some(index) = canonical_array_index_key(state, key) {
            index_keys.push((index, key, flags));
        } else {
            string_keys.push((key, flags));
        }
    }
    index_keys.sort_by_key(|(index, _, _)| *index);
    let mut properties =
        Vec::with_capacity(index_keys.len() + string_keys.len() + symbol_keys.len());
    for (_, key, _flags) in index_keys {
        let property = state
            .gc
            .heap()
            .get_property_slot(handle, key)
            .ok()
            .flatten()?;
        properties.push((
            super::runtime::encoded_property_key(key),
            property.value as i64,
        ));
    }
    for (key, _flags) in string_keys {
        let property = state
            .gc
            .heap()
            .get_property_slot(handle, key)
            .ok()
            .flatten()?;
        properties.push((
            super::runtime::encoded_property_key(key),
            property.value as i64,
        ));
    }
    for (key, _flags) in symbol_keys {
        let property = state
            .gc
            .heap()
            .get_property_slot(handle, key)
            .ok()
            .flatten()?;
        properties.push((
            super::runtime::encoded_property_key(key),
            property.value as i64,
        ));
    }
    Some(properties)
}

fn enumerate(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    kind: EnumerationKind,
) -> i64 {
    let Some(object) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    if value::is_proxy(object) {
        let keys = match super::proxy::own_keys(ctx, state, object) {
            Ok(keys) => keys,
            Err(exception) => return exception,
        };
        let mut values = Vec::with_capacity(keys.len());
        for key in keys {
            let property_value = if matches!(kind, EnumerationKind::Names) {
                key
            } else {
                let descriptor = super::proxy::get_own_property_descriptor(ctx, state, object, key);
                if value::is_exception(descriptor) {
                    return descriptor;
                }
                if value::is_undefined(descriptor)
                    || !descriptor_field(state, value::decode_handle(descriptor), "enumerable")
                        .is_some_and(|value| super::runtime::is_truthy(state, value))
                {
                    continue;
                }
                if matches!(kind, EnumerationKind::Keys) {
                    key
                } else {
                    super::runtime::get_property(ctx, state, object, key)
                        .unwrap_or_else(|()| fail_dispatch(ctx))
                }
            };
            match kind {
                EnumerationKind::Keys | EnumerationKind::Names | EnumerationKind::Values => {
                    values.push(property_value)
                }
                EnumerationKind::Entries => {
                    let Ok(entry) =
                        state.allocate_array_values_with_gc_retry(ctx, &[key, property_value])
                    else {
                        return fail_dispatch(ctx);
                    };
                    values.push(entry);
                }
            }
        }
        return state
            .allocate_array_values_with_gc_retry(ctx, &values)
            .unwrap_or_else(|_| fail_dispatch(ctx));
    }
    let Some(properties) = own_keys(state, object, !matches!(kind, EnumerationKind::Names)) else {
        return fail_dispatch(ctx);
    };
    // Object.keys 对模块命名空间也要逐键 [[GetOwnProperty]]（§7.3.24 步骤
    // 4.a），其内部 [[Get]] 对未初始化导出抛 ReferenceError（循环导入窗口，
    // 与 Node 一致）；getOwnPropertyNames（Names）只走 [[OwnPropertyKeys]]，
    // 不触发取值。
    let keys_via_get = matches!(kind, EnumerationKind::Keys)
        && value::is_object(object)
        && state
            .module_namespace_objects
            .contains(&value::decode_handle(object));
    let mut values = Vec::with_capacity(properties.len());
    for (key, _) in properties {
        if value::is_symbol(key) {
            continue;
        }
        match kind {
            EnumerationKind::Keys if keys_via_get => {
                let live = match super::runtime::get_property(ctx, state, object, key) {
                    Ok(live) => live,
                    Err(()) => return fail_dispatch(ctx),
                };
                if value::is_exception(live) {
                    return live;
                }
                values.push(key);
            }
            EnumerationKind::Keys | EnumerationKind::Names => values.push(key),
            EnumerationKind::Values | EnumerationKind::Entries => {
                // EnumerableOwnPropertyNames（§7.3.24）步骤 4.a.ii.2.a 对
                // values/entries 逐键 ? Get(O, key)：访问器槽（含模块命名
                // 空间的 live binding getter）必须经 [[Get]] 取实时值。
                let live = match super::runtime::get_property(ctx, state, object, key) {
                    Ok(live) => live,
                    Err(()) => return fail_dispatch(ctx),
                };
                if value::is_exception(live) {
                    return live;
                }
                if matches!(kind, EnumerationKind::Values) {
                    values.push(live);
                } else {
                    let Ok(entry) = state.allocate_array_values_with_gc_retry(ctx, &[key, live])
                    else {
                        return fail_dispatch(ctx);
                    };
                    values.push(entry);
                }
            }
        }
    }
    state
        .allocate_array_values_with_gc_retry(ctx, &values)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn from_entries(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(source) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let iterator = iterator_from(ctx, state, &[source]);
    if value::is_exception(iterator) {
        return iterator;
    }
    let Ok(result) = state.allocate_object_with_gc_retry(ctx, 4, false) else {
        return fail_dispatch(ctx);
    };
    loop {
        let done = iterator_done(ctx, state, &[iterator]);
        if value::is_exception(done) {
            return done;
        }
        if super::runtime::is_truthy(state, done) {
            return result;
        }
        let entry = iterator_value(ctx, state, &[iterator], true);
        if value::is_exception(entry) {
            return entry;
        }
        if !(value::is_object(entry) || value::is_array(entry)) {
            return type_error(
                ctx,
                state,
                "Object.fromEntries iterator value is not an object",
            );
        }
        let key = match get_property(ctx, state, entry, value::encode_f64(0.0)) {
            Ok(key) => key,
            Err(()) => return fail_dispatch(ctx),
        };
        let stored = match get_property(ctx, state, entry, value::encode_f64(1.0)) {
            Ok(stored) => stored,
            Err(()) => return fail_dispatch(ctx),
        };
        let Some(key) = property_key(state, key) else {
            return fail_dispatch(ctx);
        };
        if state
            .gc
            .heap()
            .set_property(value::decode_handle(result), key, stored as u64)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
}

fn group_by(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [source, callback] = args else {
        return fail_dispatch(ctx);
    };
    if value::is_null(*source) || value::is_undefined(*source) {
        return type_error(ctx, state, "Cannot group null or undefined");
    }
    if !state.is_callable_value(*callback) {
        return type_error(ctx, state, "callbackfn is not callable");
    }
    let iterator = iterator_from(ctx, state, &[*source]);
    if value::is_exception(iterator) {
        return iterator;
    }
    let Ok(result) = state.allocate_object_with_gc_retry(ctx, 4, false) else {
        return fail_dispatch(ctx);
    };
    if state
        .gc
        .heap()
        .set_prototype(value::decode_handle(result), PROTO_NULL_SENTINEL)
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    let mut index = 0_u64;
    loop {
        let done = iterator_done(ctx, state, &[iterator]);
        if value::is_exception(done) {
            return done;
        }
        if super::runtime::is_truthy(state, done) {
            return result;
        }
        let stored = iterator_value(ctx, state, &[iterator], true);
        if value::is_exception(stored) {
            return stored;
        }
        let key = state
            .invoke_callable(
                ctx,
                *callback,
                value::encode_undefined(),
                &[stored, value::encode_f64(index as f64)],
            )
            .unwrap_or_else(|| fail_dispatch(ctx));
        if value::is_exception(key) {
            return key;
        }
        let Some(key) = property_key(state, key) else {
            return fail_dispatch(ctx);
        };
        let result_handle = value::decode_handle(result);
        let group = match state.gc.heap().get_property(result_handle, key) {
            Ok(Some(group)) => group as i64,
            Ok(None) => {
                let Ok(group) = state.allocate_array_values_with_gc_retry(ctx, &[]) else {
                    return fail_dispatch(ctx);
                };
                if state
                    .gc
                    .heap()
                    .set_property(result_handle, key, group as u64)
                    .is_err()
                {
                    return fail_dispatch(ctx);
                }
                group
            }
            Err(_) => return fail_dispatch(ctx),
        };
        if state
            .gc
            .heap()
            .push_element(value::decode_handle(group), stored as u64)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
        index += 1;
    }
}

fn object_rest(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [source, excluded] = args else {
        return fail_dispatch(ctx);
    };
    if value::is_null(*source) || value::is_undefined(*source) {
        return type_error(ctx, state, "cannot destructure null or undefined");
    }
    if !value::is_array(*excluded) {
        return fail_dispatch(ctx);
    }
    let excluded_handle = value::decode_handle(*excluded);
    let Ok(excluded_len) = state.gc.heap().array_length(excluded_handle) else {
        return fail_dispatch(ctx);
    };
    let mut excluded_keys = Vec::with_capacity(excluded_len as usize);
    for index in 0..excluded_len {
        let Ok(Some(stored)) = state.gc.heap().get_element(excluded_handle, index) else {
            return fail_dispatch(ctx);
        };
        let stored = stored as i64;
        if !value::is_array_hole(stored) {
            excluded_keys.push(stored);
        }
    }
    let Ok(result) = state.allocate_object_with_gc_retry(ctx, 4, false) else {
        return fail_dispatch(ctx);
    };
    match copy_data_properties(ctx, state, result, *source, &excluded_keys) {
        Ok(()) => result,
        Err(exception) => exception,
    }
}

pub(crate) fn copy_data_properties(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    destination: i64,
    source: i64,
    excluded: &[i64],
) -> Result<(), i64> {
    let Some(destination_handle) = object_handle(destination) else {
        return Err(fail_dispatch(ctx));
    };
    let keys = enumerable_own_keys(ctx, state, source)?;
    for key in keys {
        if excluded
            .iter()
            .any(|excluded_key| strict_equal(state, key, *excluded_key))
        {
            continue;
        }
        let stored = get_property(ctx, state, source, key).map_err(|()| fail_dispatch(ctx))?;
        if value::is_exception(stored) {
            return Err(stored);
        }
        let Some(key) = property_key(state, key) else {
            return Err(fail_dispatch(ctx));
        };
        state
            .gc
            .heap()
            .set_property(destination_handle, key, stored as u64)
            .map_err(|_| fail_dispatch(ctx))?;
    }
    Ok(())
}

fn enumerable_own_keys(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    source: i64,
) -> Result<Vec<i64>, i64> {
    if value::is_proxy(source) {
        let keys = super::proxy::own_keys(ctx, state, source)?;
        let mut enumerable = Vec::with_capacity(keys.len());
        for key in keys {
            let descriptor = super::proxy::get_own_property_descriptor(ctx, state, source, key);
            if value::is_exception(descriptor) {
                return Err(descriptor);
            }
            if value::is_undefined(descriptor) {
                continue;
            }
            let Some(descriptor_handle) = object_handle(descriptor) else {
                return Err(fail_dispatch(ctx));
            };
            if read_descriptor(ctx, state, descriptor_handle)?.enumerable == Some(true) {
                enumerable.push(key);
            }
        }
        return Ok(enumerable);
    }
    if let Some(properties) = own_keys(state, source, true) {
        return Ok(properties.into_iter().map(|(key, _)| key).collect());
    }
    if value::is_null(source) || value::is_undefined(source) {
        return Err(type_error(
            ctx,
            state,
            "cannot convert null or undefined to object",
        ));
    }
    Ok(Vec::new())
}

fn assign(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(target) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    if object_handle(target).is_none() {
        return fail_dispatch(ctx);
    }
    for source in &args[1..] {
        if value::is_null(*source) || value::is_undefined(*source) {
            continue;
        }
        let Some(properties) = own_keys(state, *source, true) else {
            continue;
        };
        for (property, _) in properties {
            let stored = match get_property(ctx, state, *source, property) {
                Ok(value) => value,
                Err(()) => return fail_dispatch(ctx),
            };
            if value::is_exception(stored) {
                return stored;
            }
            // Set(to, key, value, true)：写失败按 throw=true 升级 TypeError，
            // 消息与赋值点 strict 失败同口径。
            match ordinary_set(ctx, state, target, property, stored, target) {
                Ok(super::property_write::SetCompletion::Written) => {}
                Ok(super::property_write::SetCompletion::Failed(failure)) => {
                    return super::property_write::strict_set_failure_error(
                        ctx, state, target, property, failure,
                    );
                }
                Err(exception) => return exception,
            }
        }
    }
    target
}

fn create(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(prototype) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    // OrdinaryObjectCreate（§10.1.12）：proto 槽可承载普通对象 / 数组 /
    // Proxy / RegExp（标记位编码）。
    let Some(prototype) = super::runtime::encode_proto_slot(prototype) else {
        return type_error(ctx, state, "Object prototype may only be an Object or null");
    };
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 4, false) else {
        return fail_dispatch(ctx);
    };
    if state
        .gc
        .heap()
        .set_prototype(value::decode_handle(object), prototype)
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    if let Some(descriptors) = args.get(1).copied()
        && !value::is_undefined(descriptors)
    {
        return define_properties(ctx, state, &[object, descriptors]);
    }
    object
}

pub(super) fn get_prototype(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(object) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    if value::is_regexp(object) {
        return state.regexp_prototype.unwrap_or_else(|| fail_dispatch(ctx));
    }
    if value::is_proxy(object) {
        return super::proxy::get_prototype(ctx, state, object);
    }
    if value::is_callable(object) {
        if let Some(prototype) = state
            .callable_prototypes
            .get(&value::strip_gc_color(object))
            .copied()
        {
            return prototype;
        }
        // 无显式原型的普通可调用值：[[Prototype]] 默认为 %Function.prototype%
        // （§10.2.3）；%Function.prototype% 自身的父原型是 %Object.prototype%。
        if state.native_callable_kind(object) == Some(crate::NativeCallableKind::FunctionPrototype)
        {
            return state
                .ensure_intrinsic_prototypes()
                .ok()
                .and_then(|_| state.object_prototype)
                .unwrap_or_else(value::encode_null);
        }
        return state
            .native_callable(crate::NativeCallableKind::FunctionPrototype)
            .unwrap_or_else(value::encode_null);
    }
    let Some(handle) = object_handle(object) else {
        return fail_dispatch(ctx);
    };
    match state.gc.heap().prototype(handle) {
        Ok(prototype) => {
            super::runtime::decode_proto_slot(state, prototype).unwrap_or_else(value::encode_null)
        }
        Err(_) => fail_dispatch(ctx),
    }
}

pub(super) fn set_prototype(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [object, prototype] = args else {
        return fail_dispatch(ctx);
    };
    if !(value::is_null(*prototype)
        || value::is_object(*prototype)
        || value::is_array(*prototype)
        || value::is_callable(*prototype)
        || value::is_proxy(*prototype)
        || value::is_regexp(*prototype))
    {
        return type_error(ctx, state, "Object prototype may only be an Object or null");
    }
    if value::is_proxy(*object) {
        let result = super::proxy::set_prototype(ctx, state, *object, *prototype);
        return if value::is_exception(result) {
            result
        } else if super::runtime::is_truthy(state, result) {
            *object
        } else {
            type_error(ctx, state, "Proxy rejected prototype mutation")
        };
    }
    if value::is_callable(*object) {
        // callable 无显式条目表示隐式 Function.prototype 而非 null：显式改设
        // （含 null）必须落表，[[Get]]/[[Set]]/[[HasProperty]] 的链查找才能
        // 按 OrdinaryGet 终止；仅当已有显式条目且与新原型一致时短路。
        let callable = value::strip_gc_color(*object);
        if state.callable_prototypes.get(&callable).copied() == Some(*prototype) {
            return *object;
        }
        if !value::is_null(*prototype) && state.prototype_chain_contains_value(*prototype, *object)
        {
            return type_error(ctx, state, "Cyclic __proto__ value");
        }
        state.callable_prototypes.insert(callable, *prototype);
        return *object;
    }
    let handle = object_handle(*object);
    let current = if let Some(handle) = handle {
        match state.gc.heap().prototype(handle) {
            Ok(prototype) => super::runtime::decode_proto_slot(state, prototype)
                .unwrap_or_else(value::encode_null),
            Err(_) => return fail_dispatch(ctx),
        }
    } else {
        return type_error(ctx, state, "Object.setPrototypeOf target is not an object");
    };
    if current == *prototype {
        return *object;
    }
    // Module Namespace 的 [[SetPrototypeOf]]（§10.4.6.1 SetImmutablePrototype）：
    // V 与当前原型（null）相同已在上方短路返回 true，其余一律失败；
    // Object.setPrototypeOf 抛 V8 口径 "[object Module] is not extensible"。
    if handle.is_some_and(|handle| state.module_namespace_objects.contains(&handle)) {
        return type_error(ctx, state, "[object Module] is not extensible");
    }
    if handle.is_some_and(|handle| state.non_extensible_objects.contains(&handle)) {
        return type_error(
            ctx,
            state,
            "Cannot set prototype of a non-extensible object",
        );
    }
    if !value::is_null(*prototype) && state.prototype_chain_contains_value(*prototype, *object) {
        return type_error(ctx, state, "Cyclic __proto__ value");
    }
    // callable 原型无法进 proto 槽（closure / 侧表值），保持既有句柄写入
    // 路径（类继承经 callable_prototypes 不走此处）。
    let prototype = super::runtime::encode_proto_slot(*prototype)
        .unwrap_or_else(|| value::decode_handle(*prototype));
    state
        .gc
        .heap()
        .set_prototype(
            handle.expect("ordinary object handle was checked"),
            prototype,
        )
        .map(|()| *object)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn object_is(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let [left, right] = args else {
        return fail_dispatch(ctx);
    };
    let equal = if value::is_f64(*left) && value::is_f64(*right) {
        let left = value::decode_f64(*left);
        let right = value::decode_f64(*right);
        (left.is_nan() && right.is_nan())
            || (left == right
                && (left != 0.0 || left.is_sign_positive() == right.is_sign_positive()))
    } else {
        strict_equal(state, *left, *right)
    };
    value::encode_bool(equal)
}

pub(crate) fn get_own_property_descriptor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [object, key] = args else {
        return fail_dispatch(ctx);
    };
    if value::is_proxy(*object) {
        return super::proxy::get_own_property_descriptor(ctx, state, *object, *key);
    }
    let encoded_key = *key;
    let Some(key) = property_key(state, encoded_key) else {
        return fail_dispatch(ctx);
    };
    if value::is_array(*object) {
        let handle = value::decode_handle(*object);
        let property = if state.text_matches(encoded_key, "length") {
            let Ok(length) = state.gc.heap().array_length(handle) else {
                return fail_dispatch(ctx);
            };
            // Object.freeze 后 length writable=false（configurable 恒 false）。
            let flags = if state.array_fixed_length.contains(&handle) {
                0
            } else {
                WRITABLE
            };
            Some(wjsm_gc::HeapAccessV2Property {
                flags,
                value: value::encode_f64(f64::from(length)) as u64,
                getter: value::encode_undefined() as u64,
                setter: value::encode_undefined() as u64,
            })
        } else if let Some((getter, setter, flags)) =
            state.array_accessors.get(&(handle, key)).copied()
        {
            Some(wjsm_gc::HeapAccessV2Property {
                flags: flags | ACCESSOR,
                value: value::encode_undefined() as u64,
                getter: getter as u64,
                setter: setter as u64,
            })
        } else if let Some(stored) = state.array_properties.get(&(handle, key)).copied() {
            Some(wjsm_gc::HeapAccessV2Property {
                flags: state
                    .array_property_flags
                    .get(&(handle, key))
                    .copied()
                    .unwrap_or(WRITABLE | ENUMERABLE | CONFIGURABLE),
                value: stored as u64,
                getter: value::encode_undefined() as u64,
                setter: value::encode_undefined() as u64,
            })
        } else if let Some(index) = super::runtime::array_index(state, encoded_key) {
            match state.gc.heap().get_element(handle, index) {
                Ok(Some(element)) if !value::is_array_hole(element as i64) => {
                    // seal/freeze 只迁移特性入覆盖层，元素值仍在元素存储；
                    // 特性以覆盖层条目为准，无条目为缺省可写可配置。
                    Some(wjsm_gc::HeapAccessV2Property {
                        flags: state
                            .array_property_flags
                            .get(&(handle, key))
                            .copied()
                            .unwrap_or(WRITABLE | ENUMERABLE | CONFIGURABLE),
                        value: element,
                        getter: value::encode_undefined() as u64,
                        setter: value::encode_undefined() as u64,
                    })
                }
                Ok(_) => None,
                Err(_) => return fail_dispatch(ctx),
            }
        } else if state.array_prototype == Some(*object) {
            state
                .primitive_property(*object, encoded_key)
                .map(|stored| wjsm_gc::HeapAccessV2Property {
                    flags: WRITABLE | CONFIGURABLE,
                    value: stored as u64,
                    getter: value::encode_undefined() as u64,
                    setter: value::encode_undefined() as u64,
                })
        } else {
            None
        };
        return property.map_or_else(value::encode_undefined, |property| {
            descriptor_object(ctx, state, property)
        });
    }
    if value::is_callable(*object) {
        let accessor = state.callable_accessors.get(&(*object, key)).copied();
        let stored = if accessor.is_none() {
            state.callable_property(*object, key)
        } else {
            None
        };
        let flags = state
            .callable_property_flags
            .get(&(*object, key))
            .copied()
            .unwrap_or_default();
        let property = if let Some((getter, setter)) = accessor {
            wjsm_gc::HeapAccessV2Property {
                flags: flags | ACCESSOR,
                value: u64::from_ne_bytes(value::encode_undefined().to_ne_bytes()),
                getter: u64::from_ne_bytes(getter.to_ne_bytes()),
                setter: u64::from_ne_bytes(setter.to_ne_bytes()),
            }
        } else if let Some(stored) = stored {
            wjsm_gc::HeapAccessV2Property {
                flags,
                value: u64::from_ne_bytes(stored.to_ne_bytes()),
                getter: u64::from_ne_bytes(value::encode_undefined().to_ne_bytes()),
                setter: u64::from_ne_bytes(value::encode_undefined().to_ne_bytes()),
            }
        } else {
            return value::encode_undefined();
        };
        return descriptor_object(ctx, state, property);
    }
    let Some(handle) = object_handle(*object) else {
        return super::runtime::type_error(
            ctx,
            state,
            "Object.getOwnPropertyDescriptor called on non-object",
        );
    };
    let Ok(Some(property)) = state.gc.heap().get_property_slot(handle, key) else {
        return value::encode_undefined();
    };
    // Module Namespace 的 [[GetOwnProperty]]（§10.4.6.4）：字符串键导出对外
    // 呈现为 { value: [[Get]](P), writable: true, enumerable: true,
    // configurable: false } 数据描述符（内部 live binding getter 不可见）；
    // 符号键（@@toStringTag）走 ordinary。未初始化导出（循环导入窗口）按
    // V8 口径抛 ReferenceError "{key} is not defined"（getter 只可能抛 TDZ）。
    if !key.is_symbol() && state.module_namespace_objects.contains(&handle) {
        let live = match namespace_export_live_value(ctx, state, *object, property) {
            Ok(live) => live,
            Err(_) => {
                let rendered =
                    super::runtime::render_value(state, super::runtime::encoded_property_key(key));
                return super::runtime::reference_error(
                    ctx,
                    state,
                    &format!("{rendered} is not defined"),
                );
            }
        };
        return descriptor_object(
            ctx,
            state,
            wjsm_gc::HeapAccessV2Property {
                flags: WRITABLE | ENUMERABLE,
                value: live as u64,
                getter: value::encode_undefined() as u64,
                setter: value::encode_undefined() as u64,
            },
        );
    }
    descriptor_object(ctx, state, property)
}

/// 命名空间导出槽的 live 值（§10.4.6.8 [[Get]]）：accessor 槽经 live binding
/// getter 调用取当前值；`Err` 携带 getter 抛出的异常（循环导入 TDZ 的
/// ReferenceError）。
fn namespace_export_live_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    property: wjsm_gc::HeapAccessV2Property,
) -> Result<i64, i64> {
    if property.flags & ACCESSOR == 0 {
        return Ok(property.value as i64);
    }
    let getter = property.getter as i64;
    if !value::is_callable(getter) {
        return Ok(value::encode_undefined());
    }
    let result = state
        .invoke_callable(ctx, getter, object, &[])
        .ok_or_else(|| fail_dispatch(ctx))?;
    if value::is_exception(result) {
        return Err(result);
    }
    Ok(result)
}

fn descriptor_object(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    property: wjsm_gc::HeapAccessV2Property,
) -> i64 {
    let Ok(descriptor) = state.allocate_object_with_gc_retry(ctx, 5, false) else {
        return fail_dispatch(ctx);
    };
    let handle = value::decode_handle(descriptor);
    let configurable = value::encode_bool(property.flags & CONFIGURABLE != 0);
    let enumerable = value::encode_bool(property.flags & ENUMERABLE != 0);
    let fields = if property.flags & ACCESSOR != 0 {
        vec![
            ("get", property.getter as i64),
            ("set", property.setter as i64),
            ("enumerable", enumerable),
            ("configurable", configurable),
        ]
    } else {
        vec![
            ("value", property.value as i64),
            (
                "writable",
                value::encode_bool(property.flags & WRITABLE != 0),
            ),
            ("enumerable", enumerable),
            ("configurable", configurable),
        ]
    };
    for (name, stored) in fields {
        let Some(key) = state.intern_property_string((*name).into()) else {
            return fail_dispatch(ctx);
        };
        if state
            .gc
            .heap()
            .set_property(handle, key, stored as u64)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    descriptor
}

/// 构造 CreateDataProperty 的完整数据描述符对象
/// { value, writable: true, enumerable: true, configurable: true }，
/// 供 Proxy receiver 的 [[DefineOwnProperty]] trap 消费。
pub(super) fn full_data_descriptor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    stored: i64,
) -> i64 {
    descriptor_object(
        ctx,
        state,
        wjsm_gc::HeapAccessV2Property {
            flags: WRITABLE | ENUMERABLE | CONFIGURABLE,
            value: u64::from_ne_bytes(stored.to_ne_bytes()),
            getter: u64::from_ne_bytes(value::encode_undefined().to_ne_bytes()),
            setter: u64::from_ne_bytes(value::encode_undefined().to_ne_bytes()),
        },
    )
}

fn descriptor_field(state: &mut NativeAgentState, descriptor: u32, name: &str) -> Option<i64> {
    let key = state.intern_property_string(name.into())?;
    state
        .gc
        .heap()
        .get_property(descriptor, key)
        .ok()
        .flatten()
        .map(|stored| stored as i64)
}

#[derive(Clone, Copy)]
pub(crate) struct PropertyDescriptor {
    pub(crate) configurable: Option<bool>,
    pub(crate) enumerable: Option<bool>,
    pub(crate) writable: Option<bool>,
    pub(crate) value: Option<i64>,
    pub(crate) getter: Option<i64>,
    pub(crate) setter: Option<i64>,
}

impl PropertyDescriptor {
    pub(crate) fn is_accessor(self) -> bool {
        self.getter.is_some() || self.setter.is_some()
    }

    pub(crate) fn is_data(self) -> bool {
        self.value.is_some() || self.writable.is_some()
    }
}
pub(crate) fn read_descriptor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    descriptor: u32,
) -> Result<PropertyDescriptor, i64> {
    let configurable = descriptor_field(state, descriptor, "configurable")
        .map(|stored| super::runtime::is_truthy(state, stored));
    let enumerable = descriptor_field(state, descriptor, "enumerable")
        .map(|stored| super::runtime::is_truthy(state, stored));
    let writable = descriptor_field(state, descriptor, "writable")
        .map(|stored| super::runtime::is_truthy(state, stored));
    let descriptor = PropertyDescriptor {
        configurable,
        enumerable,
        writable,
        value: descriptor_field(state, descriptor, "value"),
        getter: descriptor_field(state, descriptor, "get"),
        setter: descriptor_field(state, descriptor, "set"),
    };
    if descriptor.is_accessor() && descriptor.is_data() {
        return Err(super::runtime::type_error(
            ctx,
            state,
            "Invalid property descriptor: cannot specify accessors and a value or writable attribute",
        ));
    }
    if descriptor
        .getter
        .is_some_and(|getter| !value::is_undefined(getter) && !value::is_callable(getter))
    {
        return Err(super::runtime::type_error(
            ctx,
            state,
            "property getter must be callable",
        ));
    }
    if descriptor
        .setter
        .is_some_and(|setter| !value::is_undefined(setter) && !value::is_callable(setter))
    {
        return Err(super::runtime::type_error(
            ctx,
            state,
            "property setter must be callable",
        ));
    }
    Ok(descriptor)
}

pub(crate) fn complete_descriptor_object(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    descriptor: PropertyDescriptor,
) -> i64 {
    let mut flags = 0;
    set_flag(
        &mut flags,
        CONFIGURABLE,
        Some(descriptor.configurable.unwrap_or(false)),
    );
    set_flag(
        &mut flags,
        ENUMERABLE,
        Some(descriptor.enumerable.unwrap_or(false)),
    );
    let property = if descriptor.is_accessor() {
        wjsm_gc::HeapAccessV2Property {
            flags: flags | ACCESSOR,
            value: value::encode_undefined() as u64,
            getter: descriptor.getter.unwrap_or_else(value::encode_undefined) as u64,
            setter: descriptor.setter.unwrap_or_else(value::encode_undefined) as u64,
        }
    } else {
        set_flag(
            &mut flags,
            WRITABLE,
            Some(descriptor.writable.unwrap_or(false)),
        );
        wjsm_gc::HeapAccessV2Property {
            flags,
            value: descriptor.value.unwrap_or_else(value::encode_undefined) as u64,
            getter: value::encode_undefined() as u64,
            setter: value::encode_undefined() as u64,
        }
    };
    descriptor_object(ctx, state, property)
}
pub(crate) fn descriptor_is_compatible(
    state: &NativeAgentState,
    descriptor: PropertyDescriptor,
    current: PropertyDescriptor,
) -> bool {
    if current.configurable == Some(false) {
        if descriptor.configurable == Some(true)
            || descriptor
                .enumerable
                .is_some_and(|enumerable| Some(enumerable) != current.enumerable)
            || descriptor.is_accessor() != current.is_accessor()
                && (descriptor.is_accessor() || descriptor.is_data())
        {
            return false;
        }
        if current.is_data()
            && current.writable == Some(false)
            && (descriptor.writable == Some(true)
                || descriptor.value.is_some_and(|stored| {
                    current
                        .value
                        .is_none_or(|current| !same_value(state, stored, current))
                }))
        {
            return false;
        }
        if current.is_accessor()
            && (descriptor.getter.is_some_and(|getter| {
                current
                    .getter
                    .is_none_or(|current| !same_value(state, getter, current))
            }) || descriptor.setter.is_some_and(|setter| {
                current
                    .setter
                    .is_none_or(|current| !same_value(state, setter, current))
            }))
        {
            return false;
        }
    }
    true
}

fn define_ordinary_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    handle: u32,
    key: PropertyKey,
    descriptor_handle: u32,
) -> i64 {
    let descriptor = match read_descriptor(ctx, state, descriptor_handle) {
        Ok(descriptor) => descriptor,
        Err(exception) => return exception,
    };
    let current = match state.gc.heap().get_property_slot(handle, key) {
        Ok(current) => current,
        Err(_) => return fail_dispatch(ctx),
    };
    if current.is_none() && state.non_extensible_objects.contains(&handle) {
        // V8 文案：`Cannot define property <key>, object is not extensible`。
        let rendered =
            super::runtime::render_value(state, super::runtime::encoded_property_key(key));
        return super::runtime::type_error(
            ctx,
            state,
            &format!("Cannot define property {rendered}, object is not extensible"),
        );
    }

    if let Some(current) = current
        && current.flags & CONFIGURABLE == 0
    {
        if descriptor.configurable == Some(true)
            || descriptor
                .enumerable
                .is_some_and(|enumerable| enumerable != (current.flags & ENUMERABLE != 0))
            || descriptor.is_accessor() != (current.flags & ACCESSOR != 0)
                && (descriptor.is_accessor() || descriptor.is_data())
        {
            return incompatible_descriptor(ctx, state);
        }
        if current.flags & ACCESSOR == 0
            && current.flags & WRITABLE == 0
            && (descriptor.writable == Some(true)
                || descriptor
                    .value
                    .is_some_and(|stored| !same_value(state, stored, current.value as i64)))
        {
            return incompatible_descriptor(ctx, state);
        }
        if current.flags & ACCESSOR != 0
            && (descriptor
                .getter
                .is_some_and(|getter| !same_value(state, getter, current.getter as i64))
                || descriptor
                    .setter
                    .is_some_and(|setter| !same_value(state, setter, current.setter as i64)))
        {
            return incompatible_descriptor(ctx, state);
        }
    }

    // mapped arguments（ES §10.4.4.2）：define 成功后按描述符种类维护
    // [[ParameterMap]]。`previous` 取 define 前的属性值——映射期间该值即形参
    // 绑定真值，访问器降级解除映射时作为绑定快照保留。
    let mapped_slot = super::arguments::live_mapped_index(state, handle, key).map(|index| {
        (
            index,
            current.map_or_else(value::encode_undefined, |current| current.value as i64),
        )
    });
    let current_is_accessor = current.is_some_and(|current| current.flags & ACCESSOR != 0);
    let use_accessor = if descriptor.is_accessor() {
        true
    } else if descriptor.is_data() {
        false
    } else {
        current_is_accessor
    };
    let switching_kind = current.is_some() && use_accessor != current_is_accessor;
    let mut flags = if switching_kind {
        0
    } else {
        current.map_or(0, |current| current.flags & !ACCESSOR)
    };
    set_flag(&mut flags, CONFIGURABLE, descriptor.configurable);
    set_flag(&mut flags, ENUMERABLE, descriptor.enumerable);
    let result = if use_accessor {
        let getter = descriptor.getter.unwrap_or_else(|| {
            current
                .filter(|current| current.flags & ACCESSOR != 0 && !switching_kind)
                .map_or_else(value::encode_undefined, |current| current.getter as i64)
        });
        let setter = descriptor.setter.unwrap_or_else(|| {
            current
                .filter(|current| current.flags & ACCESSOR != 0 && !switching_kind)
                .map_or_else(value::encode_undefined, |current| current.setter as i64)
        });
        match state.gc.heap().define_accessor_property_with_flags(
            handle,
            key,
            getter as u64,
            setter as u64,
            flags,
        ) {
            Ok(()) => object,
            Err(wjsm_gc::HeapAccessV2Error::NativeTlabNeedsMaterialization { .. }) => {
                if state.gc.flush_native_tlab(ctx).is_err() {
                    return fail_dispatch(ctx);
                }
                state
                    .gc
                    .heap()
                    .define_accessor_property_with_flags(
                        handle,
                        key,
                        getter as u64,
                        setter as u64,
                        flags,
                    )
                    .map(|()| object)
                    .unwrap_or_else(|_| fail_dispatch(ctx))
            }
            Err(_) => fail_dispatch(ctx),
        }
    } else {
        set_flag(&mut flags, WRITABLE, descriptor.writable);
        let stored = descriptor.value.unwrap_or_else(|| {
            current
                .filter(|current| current.flags & ACCESSOR == 0 && !switching_kind)
                .map_or_else(value::encode_undefined, |current| current.value as i64)
        });
        match state
            .gc
            .heap()
            .define_data_property(handle, key, stored as u64, flags)
        {
            Ok(()) => object,
            Err(wjsm_gc::HeapAccessV2Error::NativeTlabNeedsMaterialization { .. }) => {
                if state.gc.flush_native_tlab(ctx).is_err() {
                    return fail_dispatch(ctx);
                }
                state
                    .gc
                    .heap()
                    .define_data_property(handle, key, stored as u64, flags)
                    .map(|()| object)
                    .unwrap_or_else(|_| fail_dispatch(ctx))
            }
            Err(_) => fail_dispatch(ctx),
        }
    };
    if result == object
        && let Some((index, previous)) = mapped_slot
    {
        super::arguments::after_define_own_property(
            state,
            handle,
            index,
            previous,
            descriptor.is_accessor(),
            descriptor.writable == Some(false),
        );
    }
    result
}

fn set_flag(flags: &mut u32, bit: u32, update: Option<bool>) {
    match update {
        Some(true) => *flags |= bit,
        Some(false) => *flags &= !bit,
        None => {}
    }
}

fn incompatible_descriptor(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    super::runtime::type_error(ctx, state, "Cannot redefine non-configurable property")
}

fn same_value(state: &NativeAgentState, left: i64, right: i64) -> bool {
    if value::is_f64(left) && value::is_f64(right) {
        let left = value::decode_f64(left);
        let right = value::decode_f64(right);
        left.is_nan() && right.is_nan()
            || left == right && (left != 0.0 || left.is_sign_positive() == right.is_sign_positive())
    } else {
        strict_equal(state, left, right)
    }
}

pub(crate) fn define_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [object, key, descriptor] = args else {
        return fail_dispatch(ctx);
    };
    if value::is_proxy(*object) {
        return super::proxy::define_property(ctx, state, *object, *key, *descriptor);
    }
    let Some(descriptor) = object_handle(*descriptor) else {
        return fail_dispatch(ctx);
    };
    let encoded_key = *key;
    let Some(key) = property_key(state, encoded_key) else {
        return fail_dispatch(ctx);
    };
    if value::is_callable(*object) {
        return define_callable_property(ctx, state, *object, key, descriptor);
    }
    let Some(handle) = object_handle(*object) else {
        return fail_dispatch(ctx);
    };
    if value::is_array(*object) {
        return define_array_property(ctx, state, *object, handle, key, encoded_key, descriptor);
    }
    if state.module_namespace_objects.contains(&handle) {
        return define_namespace_property(ctx, state, *object, handle, key, descriptor);
    }
    define_ordinary_property(ctx, state, *object, handle, key, descriptor)
}

/// Module Namespace 的 [[DefineOwnProperty]]（§10.4.6.6）：
/// - 符号键走 OrdinaryDefineOwnProperty 语义（对象不可扩展 + @@toStringTag
///   全 false 数据属性）——新键按不可扩展拒绝，既有键只允许不改变任何字段；
/// - 字符串键：键不存在、访问器描述符、configurable:true、enumerable:false、
///   writable:false 一律拒绝；带 [[Value]] 时按 SameValue(值, [[Get]](P))
///   判定。成功路径不发生任何实际修改。
/// 失败按 V8 文案抛 "Cannot redefine property: {key}"（Reflect 入口由调用方
/// 转换为 false）。
fn define_namespace_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    handle: u32,
    key: PropertyKey,
    descriptor_handle: u32,
) -> i64 {
    let descriptor = match read_descriptor(ctx, state, descriptor_handle) {
        Ok(descriptor) => descriptor,
        Err(exception) => return exception,
    };
    let current = match state.gc.heap().get_property_slot(handle, key) {
        Ok(current) => current,
        Err(_) => return fail_dispatch(ctx),
    };
    if key.is_symbol() {
        let Some(current) = current else {
            let rendered =
                super::runtime::render_value(state, super::runtime::encoded_property_key(key));
            return type_error(
                ctx,
                state,
                &format!("Cannot define property {rendered}, object is not extensible"),
            );
        };
        let unchanged = descriptor.configurable != Some(true)
            && descriptor.enumerable != Some(true)
            && !descriptor.is_accessor()
            && descriptor.writable != Some(true)
            && descriptor
                .value
                .is_none_or(|stored| same_value(state, stored, current.value as i64));
        if unchanged {
            return object;
        }
        return namespace_redefine_error(ctx, state, key);
    }
    let Some(current) = current else {
        return namespace_redefine_error(ctx, state, key);
    };
    if descriptor.is_accessor()
        || descriptor.configurable == Some(true)
        || descriptor.enumerable == Some(false)
        || descriptor.writable == Some(false)
    {
        return namespace_redefine_error(ctx, state, key);
    }
    if let Some(stored) = descriptor.value {
        let live = match namespace_export_live_value(ctx, state, object, current) {
            Ok(live) => live,
            Err(exception) => return exception,
        };
        if !same_value(state, stored, live) {
            return namespace_redefine_error(ctx, state, key);
        }
    }
    object
}

/// 命名空间 [[DefineOwnProperty]] 拒绝的 TypeError（V8 文案）。
fn namespace_redefine_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    key: PropertyKey,
) -> i64 {
    let rendered = super::runtime::render_value(state, super::runtime::encoded_property_key(key));
    type_error(ctx, state, &format!("Cannot redefine property: {rendered}"))
}

/// callable 的 [[DefineOwnProperty]]：惰性自有属性先物化参与校验；属性缺失
/// 且不可扩展拒绝；不可配置属性按 ValidateAndApplyPropertyDescriptor 拒绝
/// 不兼容重定义；缺省特性继承既有条目（新属性缺省 false）。
fn define_callable_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    key: PropertyKey,
    descriptor_handle: u32,
) -> i64 {
    let callable = value::strip_gc_color(object);
    let descriptor = match read_descriptor(ctx, state, descriptor_handle) {
        Ok(descriptor) => descriptor,
        Err(exception) => return exception,
    };
    let _ = state.callable_property(callable, key);
    let current_accessor = state.callable_accessors.get(&(callable, key)).copied();
    let current_value = state.callable_properties.get(&(callable, key)).copied();
    let current_flags = state
        .callable_property_flags
        .get(&(callable, key))
        .copied()
        .unwrap_or(crate::ASSIGNED_PROPERTY_FLAGS);
    let exists = current_accessor.is_some() || current_value.is_some();
    if !exists {
        if state.non_extensible_callables.contains(&callable) {
            // V8 文案：`Cannot define property <key>, object is not extensible`。
            let rendered =
                super::runtime::render_value(state, super::runtime::encoded_property_key(key));
            return type_error(
                ctx,
                state,
                &format!("Cannot define property {rendered}, object is not extensible"),
            );
        }
    } else {
        let current = PropertyDescriptor {
            configurable: Some(current_flags & CONFIGURABLE != 0),
            enumerable: Some(current_flags & ENUMERABLE != 0),
            writable: current_accessor
                .is_none()
                .then_some(current_flags & WRITABLE != 0),
            value: current_value,
            getter: current_accessor.map(|(getter, _)| getter),
            setter: current_accessor.map(|(_, setter)| setter),
        };
        if !descriptor_is_compatible(state, descriptor, current) {
            return incompatible_descriptor(ctx, state);
        }
    }
    apply_callable_descriptor(state, callable, key, descriptor, exists);
    object
}

/// define_callable_property 的应用阶段：种类切换清零特性，同类更新继承
/// 既有特性；数据/访问器载荷写入侧表并触发 GC 写屏障。
fn apply_callable_descriptor(
    state: &mut NativeAgentState,
    callable: i64,
    key: PropertyKey,
    descriptor: PropertyDescriptor,
    exists: bool,
) {
    let current_accessor = state.callable_accessors.get(&(callable, key)).copied();
    let current_value = state.callable_properties.get(&(callable, key)).copied();
    let current_is_accessor = current_accessor.is_some();
    let use_accessor = if descriptor.is_accessor() {
        true
    } else if descriptor.is_data() {
        false
    } else {
        current_is_accessor
    };
    let switching_kind = exists && use_accessor != current_is_accessor;
    let mut flags = if !exists || switching_kind {
        0
    } else {
        state
            .callable_property_flags
            .get(&(callable, key))
            .copied()
            .unwrap_or(crate::ASSIGNED_PROPERTY_FLAGS)
    };
    set_flag(&mut flags, CONFIGURABLE, descriptor.configurable);
    set_flag(&mut flags, ENUMERABLE, descriptor.enumerable);
    if use_accessor {
        let carried = current_accessor.filter(|_| !switching_kind);
        let next_getter = descriptor
            .getter
            .or(carried.map(|(getter, _)| getter))
            .unwrap_or_else(value::encode_undefined);
        let next_setter = descriptor
            .setter
            .or(carried.map(|(_, setter)| setter))
            .unwrap_or_else(value::encode_undefined);
        let old_property = state.callable_properties.remove(&(callable, key));
        state
            .callable_accessors
            .insert((callable, key), (next_getter, next_setter));
        state.gc.record_host_write(callable, old_property, None);
        let (old_getter, old_setter) = match current_accessor {
            Some((old_getter, old_setter)) => (Some(old_getter), Some(old_setter)),
            None => (None, None),
        };
        state
            .gc
            .record_host_write(callable, old_getter, Some(next_getter));
        state
            .gc
            .record_host_write(callable, old_setter, Some(next_setter));
        state
            .callable_property_flags
            .insert((callable, key), flags & !WRITABLE);
    } else {
        set_flag(&mut flags, WRITABLE, descriptor.writable);
        let stored = descriptor
            .value
            .or(if switching_kind { None } else { current_value })
            .unwrap_or_else(value::encode_undefined);
        let existing = state.callable_accessors.remove(&(callable, key));
        let old_property = state.callable_properties.insert((callable, key), stored);
        if let Some((old_getter, old_setter)) = existing {
            state
                .gc
                .record_host_write(callable, Some(old_getter), Some(stored));
            state
                .gc
                .record_host_write(callable, Some(old_setter), Some(stored));
        }
        state
            .gc
            .record_host_write(callable, old_property, Some(stored));
        state.callable_property_flags.insert((callable, key), flags);
    }
}

/// 数组的 [[DefineOwnProperty]]：读取现有条目（覆盖层数据 / 访问器 / 在范围
/// 元素，元素缺省可写可枚举可配置）作为 ValidateAndApplyPropertyDescriptor
/// 的 current，缺省特性继承 current（新属性缺省 false），不可配置属性按规范
/// 拒绝不兼容重定义；`length` 走 ArraySetLength 特化路径。
fn define_array_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    handle: u32,
    key: PropertyKey,
    encoded_key: i64,
    descriptor_handle: u32,
) -> i64 {
    let descriptor = match read_descriptor(ctx, state, descriptor_handle) {
        Ok(descriptor) => descriptor,
        Err(exception) => return exception,
    };
    if state.text_matches(encoded_key, "length") {
        return define_array_length(ctx, state, object, handle, descriptor);
    }
    let index = super::runtime::array_index(state, encoded_key);
    let Ok(old_length) = state.gc.heap().array_length(handle) else {
        return fail_dispatch(ctx);
    };
    // ArraySetLength 不可写 length 拒绝新增越界下标（§10.4.2.1 步骤 2.d，
    // V8 文案）。
    if let Some(index) = index
        && index >= old_length
        && state.array_fixed_length.contains(&handle)
    {
        return super::runtime::type_error(
            ctx,
            state,
            &format!("Cannot define property {index}, object is not extensible"),
        );
    }
    let current_accessor = state.array_accessors.get(&(handle, key)).copied();
    let element_value = index.and_then(|index| {
        state
            .gc
            .heap()
            .get_element(handle, index)
            .ok()
            .flatten()
            .map(|element| i64::from_ne_bytes(element.to_ne_bytes()))
            .filter(|element| !value::is_array_hole(*element))
    });
    let current_value = state
        .array_properties
        .get(&(handle, key))
        .copied()
        .or(element_value);
    // (flags, 是否访问器)：flags 条目可独立于取值存在（seal/freeze 仅迁移
    // 特性），无任何条目的在范围元素取缺省特性。
    let current = if let Some((_, _, flags)) = current_accessor {
        Some((flags, true))
    } else if let Some(flags) = state.array_property_flags.get(&(handle, key)).copied() {
        Some((flags, false))
    } else if current_value.is_some() {
        Some((crate::ASSIGNED_PROPERTY_FLAGS, false))
    } else {
        None
    };
    if current.is_none() && state.non_extensible_objects.contains(&handle) {
        let rendered = super::runtime::render_value(state, encoded_key);
        return super::runtime::type_error(
            ctx,
            state,
            &format!("Cannot define property {rendered}, object is not extensible"),
        );
    }
    if let Some((current_flags, current_is_accessor)) = current
        && current_flags & CONFIGURABLE == 0
    {
        if descriptor.configurable == Some(true)
            || descriptor
                .enumerable
                .is_some_and(|enumerable| enumerable != (current_flags & ENUMERABLE != 0))
            || descriptor.is_accessor() != current_is_accessor
                && (descriptor.is_accessor() || descriptor.is_data())
        {
            return incompatible_descriptor(ctx, state);
        }
        if !current_is_accessor
            && current_flags & WRITABLE == 0
            && (descriptor.writable == Some(true)
                || descriptor.value.is_some_and(|stored| {
                    !same_value(
                        state,
                        stored,
                        current_value.unwrap_or_else(value::encode_undefined),
                    )
                }))
        {
            return incompatible_descriptor(ctx, state);
        }
        if current_is_accessor
            && let Some((getter, setter, _)) = current_accessor
            && (descriptor
                .getter
                .is_some_and(|next| !same_value(state, next, getter))
                || descriptor
                    .setter
                    .is_some_and(|next| !same_value(state, next, setter)))
        {
            return incompatible_descriptor(ctx, state);
        }
    }
    if index.is_some()
        && state
            .gc
            .heap()
            .raise_array_kind(handle, wjsm_ir::constants::ARRAY_KIND_DICTIONARY)
            .is_err()
    {
        return fail_dispatch(ctx);
    }
    let current_is_accessor = current.is_some_and(|(_, accessor)| accessor);
    let use_accessor = if descriptor.is_accessor() {
        true
    } else if descriptor.is_data() {
        false
    } else {
        current_is_accessor
    };
    let switching_kind = current.is_some() && use_accessor != current_is_accessor;
    let mut flags = if switching_kind {
        0
    } else {
        current.map_or(0, |(flags, _)| flags)
    };
    set_flag(&mut flags, CONFIGURABLE, descriptor.configurable);
    set_flag(&mut flags, ENUMERABLE, descriptor.enumerable);
    state.note_array_property(handle, key);
    if use_accessor {
        let existing = (!switching_kind)
            .then_some(current_accessor)
            .flatten()
            .map(|(getter, setter, _)| (getter, setter));
        state.array_properties.remove(&(handle, key));
        state.array_property_flags.remove(&(handle, key));
        state.array_accessors.insert(
            (handle, key),
            (
                descriptor
                    .getter
                    .or_else(|| existing.map(|(getter, _)| getter))
                    .unwrap_or_else(value::encode_undefined),
                descriptor
                    .setter
                    .or_else(|| existing.map(|(_, setter)| setter))
                    .unwrap_or_else(value::encode_undefined),
                flags,
            ),
        );
        // §10.4.2.1 步骤 2.g：新增越界下标定义成功后 length 提升为 index+1。
        if let Some(index) = index
            && index >= old_length
            && state.gc.heap().set_array_length(handle, index + 1).is_err()
        {
            return fail_dispatch(ctx);
        }
        return object;
    }
    set_flag(&mut flags, WRITABLE, descriptor.writable);
    let stored = descriptor
        .value
        .or(if switching_kind { None } else { current_value })
        .unwrap_or_else(value::encode_undefined);
    state.array_accessors.remove(&(handle, key));
    state.array_properties.insert((handle, key), stored);
    state.array_property_flags.insert((handle, key), flags);
    // 在范围下标同步元素存储，保持 render / 迭代等直读路径与 [[Get]] 一致。
    if let Some(index) = index
        && index < old_length
    {
        let _ =
            state
                .gc
                .heap()
                .set_element(handle, index, u64::from_ne_bytes(stored.to_ne_bytes()));
    }
    // §10.4.2.1 步骤 2.g：新增越界下标定义成功后 length 提升为 index+1。
    if let Some(index) = index
        && index >= old_length
        && state.gc.heap().set_array_length(handle, index + 1).is_err()
    {
        return fail_dispatch(ctx);
    }
    object
}

/// 数组 `length` 的 [[DefineOwnProperty]]（ArraySetLength）：length 恒不可
/// 配置不可枚举；writable false→true 与 configurable/enumerable 提升、访问器
/// 化一律拒绝；带 value 时按收缩语义执行（不可配置元素阻塞收缩同样拒绝），
/// 但对不可写 length 的 SameValue 重定义为无操作成功（区别于 [[Set]]）；
/// writable:false 记录 length 冻结标记。
fn define_array_length(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    handle: u32,
    descriptor: PropertyDescriptor,
) -> i64 {
    if descriptor.is_accessor()
        || descriptor.configurable == Some(true)
        || descriptor.enumerable == Some(true)
        || (descriptor.writable == Some(true) && state.array_fixed_length.contains(&handle))
    {
        return incompatible_descriptor(ctx, state);
    }
    if let Some(stored) = descriptor.value {
        let Some(requested) = super::runtime::array_length(state, stored) else {
            return super::runtime::range_error(ctx, state, "Invalid array length");
        };
        if state.array_fixed_length.contains(&handle) {
            let Ok(current) = state.gc.heap().array_length(handle) else {
                return fail_dispatch(ctx);
            };
            if requested != current {
                return incompatible_descriptor(ctx, state);
            }
        } else {
            let completion =
                super::runtime::set_array_length_completion(ctx, state, object, stored);
            match completion {
                Err(exception) => return exception,
                Ok(completion) if !completion.succeeded() => {
                    return incompatible_descriptor(ctx, state);
                }
                Ok(_) => {}
            }
        }
    }
    if descriptor.writable == Some(false) {
        state.array_fixed_length.insert(handle);
    }
    object
}

fn get_own_property_descriptors(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(object) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let Some(keys) = own_keys(state, object, false) else {
        return fail_dispatch(ctx);
    };
    let Ok(result) = state.allocate_object_with_gc_retry(ctx, keys.len() as u32, false) else {
        return fail_dispatch(ctx);
    };
    let result_handle = value::decode_handle(result);
    for (key, _) in keys {
        let descriptor = get_own_property_descriptor(ctx, state, &[object, key]);
        let Some(property_key) = property_key(state, key) else {
            return fail_dispatch(ctx);
        };
        if value::is_exception(descriptor)
            || state
                .gc
                .heap()
                .set_property(result_handle, property_key, descriptor as u64)
                .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    result
}

fn define_properties(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [object, descriptors] = args else {
        return fail_dispatch(ctx);
    };
    let Some(properties) = own_keys(state, *descriptors, true) else {
        return fail_dispatch(ctx);
    };
    for (key, descriptor) in properties {
        let result = define_property(ctx, state, &[*object, key, descriptor]);
        if value::is_exception(result) {
            return result;
        }
    }
    *object
}

fn prevent_extensions(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(object) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    if value::is_proxy(object) {
        let result = super::proxy::prevent_extensions(ctx, state, object);
        return if value::is_exception(result) {
            result
        } else if super::runtime::is_truthy(state, result) {
            object
        } else {
            super::runtime::type_error(
                ctx,
                state,
                "Object.preventExtensions proxy trap returned falsy",
            )
        };
    }
    if integrity_primitive(object) {
        // Object.preventExtensions 步骤 1：非对象参数原样返回。
        return object;
    }
    if value::is_callable(object) {
        state
            .non_extensible_callables
            .insert(value::strip_gc_color(object));
        return object;
    }
    let Some(handle) = object_handle(object) else {
        return fail_dispatch(ctx);
    };
    // 数组必须升为字典种类：编译产物的 packed 元素写入 / push 快路径不检查
    // 宿主侧不可扩展表，字典种类使其退回宿主分派执行扩展性检查。
    if value::is_array(object)
        && state
            .gc
            .heap()
            .raise_array_kind(handle, wjsm_ir::constants::ARRAY_KIND_DICTIONARY)
            .is_err()
    {
        return fail_dispatch(ctx);
    }
    state.non_extensible_objects.insert(handle);
    object
}

fn is_extensible(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(object) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    if value::is_proxy(object) {
        return super::proxy::is_extensible(ctx, state, object);
    }
    if value::is_callable(object) {
        return value::encode_bool(
            !state
                .non_extensible_callables
                .contains(&value::strip_gc_color(object)),
        );
    }
    if integrity_primitive(object) {
        // Object.isExtensible 步骤 1：非对象参数恒为 false。
        return value::encode_bool(false);
    }
    let Some(handle) = object_handle(object) else {
        return fail_dispatch(ctx);
    };
    value::encode_bool(!state.non_extensible_objects.contains(&handle))
}

/// 完整性操作（freeze/seal/preventExtensions 及其谓词）对非对象参数的
/// 短路判定：基元（含 null/undefined）按 ES2015+ 语义原样处理，不进堆路径。
fn integrity_primitive(object: i64) -> bool {
    !value::is_object(object)
        && !value::is_array(object)
        && !value::is_callable(object)
        && !value::is_proxy(object)
}

fn seal_or_freeze(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    freeze: bool,
) -> i64 {
    let Some(object) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    if integrity_primitive(object) {
        // SetIntegrityLevel 入口（Object.freeze/seal 步骤 1）：非对象原样返回。
        return object;
    }
    if value::is_callable(object) {
        return match seal_or_freeze_callable(state, object, freeze) {
            Some(()) => object,
            None => fail_dispatch(ctx),
        };
    }
    let Some(handle) = object_handle(object) else {
        return fail_dispatch(ctx);
    };
    // Module Namespace：SetIntegrityLevel 对每个自有键做 DefinePropertyOrThrow。
    // seal（{configurable:false}）对导出与 @@toStringTag 均无变化、恒成功；
    // freeze（数据属性追加 writable:false）被 §10.4.6.6 步骤 7 拒绝——存在
    // 字符串导出时按 [[OwnPropertyKeys]] 序（导出名升序在符号之前）对首个
    // 导出抛 "Cannot redefine property"。两种成功路径都不改动底层槽特性，
    // [[GetOwnProperty]] 的虚拟化描述符保持 writable:true。
    if state.module_namespace_objects.contains(&handle) {
        let Ok(properties) = state.gc.heap().own_property_slots(handle) else {
            return fail_dispatch(ctx);
        };
        let first_export = properties
            .iter()
            .map(|(key, _)| *key)
            .find(|key| !key.is_symbol());
        if freeze && let Some(key) = first_export {
            return namespace_redefine_error(ctx, state, key);
        }
        return object;
    }
    if value::is_array(object) && seal_or_freeze_array(state, handle, freeze).is_none() {
        return fail_dispatch(ctx);
    }
    // mapped arguments：freeze 等价于对每个数据属性应用 writable:false 的
    // [[DefineOwnProperty]]（§10.4.4.2 步骤 7.b.ii 逐一解除映射并快照绑定）；
    // seal 只收紧 configurable，映射存续。须在收紧前快照属性值。
    if freeze {
        super::arguments::unmap_all_for_freeze(state, handle);
    }
    let Ok(properties) = state.gc.heap().own_property_slots(handle) else {
        return fail_dispatch(ctx);
    };
    for (key, flags) in properties {
        let flags = flags & !CONFIGURABLE & if freeze { !WRITABLE } else { u32::MAX };
        if state
            .gc
            .heap()
            .update_property_flags(handle, key, flags)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    state.non_extensible_objects.insert(handle);
    object
}

/// SetIntegrityLevel 的数组部分：升为字典种类后为每个非 hole 元素建立
/// 覆盖层“特性”条目（值仍由元素存储持有，避免枚举与读取路径出现双份），
/// 再统一收紧全部既有覆盖层条目（下标与命名，数据与访问器）；freeze 追加
/// length 不可写标记。
fn seal_or_freeze_array(state: &mut NativeAgentState, handle: u32, freeze: bool) -> Option<()> {
    state
        .gc
        .heap()
        .raise_array_kind(handle, wjsm_ir::constants::ARRAY_KIND_DICTIONARY)
        .ok()?;
    let length = state.gc.heap().array_length(handle).ok()?;
    for index in 0..length {
        let element = match state.gc.heap().get_element(handle, index) {
            Ok(Some(element)) => element as i64,
            // 超出已分配容量的隐式 hole 与显式 hole 均非自有属性。
            Ok(None) => continue,
            Err(_) => return None,
        };
        if value::is_array_hole(element) {
            continue;
        }
        let key = property_key(state, value::encode_f64(f64::from(index)))?;
        if state.array_accessors.contains_key(&(handle, key))
            || state.array_property_flags.contains_key(&(handle, key))
        {
            continue;
        }
        state
            .array_property_flags
            .insert((handle, key), crate::ASSIGNED_PROPERTY_FLAGS);
    }
    let strip = CONFIGURABLE | if freeze { WRITABLE } else { 0 };
    let data_keys: Vec<PropertyKey> = state
        .array_property_flags
        .keys()
        .filter(|(owner, _)| *owner == handle)
        .map(|(_, key)| *key)
        .collect();
    for key in data_keys {
        if let Some(flags) = state.array_property_flags.get_mut(&(handle, key)) {
            *flags &= !strip;
        }
    }
    let accessor_keys: Vec<PropertyKey> = state
        .array_accessors
        .keys()
        .filter(|(owner, _)| *owner == handle)
        .map(|(_, key)| *key)
        .collect();
    for key in accessor_keys {
        if let Some((_, _, flags)) = state.array_accessors.get_mut(&(handle, key)) {
            // 访问器属性无 writable 概念，freeze 同样只收紧 configurable。
            *flags &= !CONFIGURABLE;
        }
    }
    if freeze {
        state.array_fixed_length.insert(handle);
    }
    Some(())
}

/// SetIntegrityLevel 的 callable 部分：先物化 name / length / prototype 惰性
/// 自有属性使其特性条目参与收紧；无 flags 条目的自有属性先补缺省特性，再
/// 统一剥除 configurable（freeze 追加剥除 writable），最后标记不可扩展。
fn seal_or_freeze_callable(state: &mut NativeAgentState, object: i64, freeze: bool) -> Option<()> {
    let callable = value::strip_gc_color(object);
    materialize_callable_lazy_properties(state, callable)?;
    let own_keys: Vec<PropertyKey> = state
        .callable_properties
        .keys()
        .chain(state.callable_accessors.keys())
        .filter(|(owner, _)| *owner == callable)
        .map(|(_, key)| *key)
        .collect();
    let strip = CONFIGURABLE | if freeze { WRITABLE } else { 0 };
    for key in own_keys {
        let flags = state
            .callable_property_flags
            .entry((callable, key))
            .or_insert(crate::ASSIGNED_PROPERTY_FLAGS);
        *flags &= !strip;
    }
    state.non_extensible_callables.insert(callable);
    Some(())
}

/// TestIntegrityLevel 的 callable 部分（不可扩展性由调用方先验）：物化惰性
/// 自有属性后检查全部自有条目均不可配置，frozen 下数据条目还须不可写
/// （访问器无 writable 概念）。无 flags 条目视为缺省可写可配置。
fn callable_integrity_level(
    state: &mut NativeAgentState,
    callable: i64,
    frozen: bool,
) -> Option<bool> {
    materialize_callable_lazy_properties(state, callable)?;
    let tightened = |state: &NativeAgentState, key: PropertyKey, data: bool| {
        state
            .callable_property_flags
            .get(&(callable, key))
            .is_some_and(|flags| {
                flags & CONFIGURABLE == 0 && (!frozen || !data || flags & WRITABLE == 0)
            })
    };
    let data_ok = state
        .callable_properties
        .keys()
        .filter(|(owner, _)| *owner == callable)
        .all(|(_, key)| tightened(state, *key, true));
    let accessor_ok = state
        .callable_accessors
        .keys()
        .filter(|(owner, _)| *owner == callable)
        .all(|(_, key)| tightened(state, *key, false));
    Some(data_ok && accessor_ok)
}

/// 触发 callable 的 name / length / prototype 惰性自有属性物化（不适用的
/// 键各自缺席，如箭头函数无 prototype），使完整性操作可见其特性条目。
fn materialize_callable_lazy_properties(state: &mut NativeAgentState, callable: i64) -> Option<()> {
    for name in ["name", "length", "prototype"] {
        let key = state.intern_property_string(name.into())?;
        let _ = state.callable_property(callable, key);
    }
    Some(())
}

fn is_sealed_or_frozen(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    frozen: bool,
) -> i64 {
    let Some(object) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    if integrity_primitive(object) {
        // TestIntegrityLevel 入口（Object.isFrozen/isSealed 步骤 1）：非对象恒 true。
        return value::encode_bool(true);
    }
    if value::is_callable(object) {
        let callable = value::strip_gc_color(object);
        if !state.non_extensible_callables.contains(&callable) {
            return value::encode_bool(false);
        }
        return match callable_integrity_level(state, callable, frozen) {
            Some(result) => value::encode_bool(result),
            None => fail_dispatch(ctx),
        };
    }
    let Some(handle) = object_handle(object) else {
        return fail_dispatch(ctx);
    };
    if !state.non_extensible_objects.contains(&handle) {
        return value::encode_bool(false);
    }
    // Module Namespace：TestIntegrityLevel 读到的是 [[GetOwnProperty]] 虚拟化
    // 描述符——导出恒 { writable: true, configurable: false }，@@toStringTag
    // 全 false。sealed 恒 true；frozen 仅当无字符串导出。
    if state.module_namespace_objects.contains(&handle) {
        let Ok(properties) = state.gc.heap().own_property_slots(handle) else {
            return fail_dispatch(ctx);
        };
        let has_export = properties.iter().any(|(key, _)| !key.is_symbol());
        return value::encode_bool(!(frozen && has_export));
    }
    if value::is_array(object) {
        return match array_integrity_level(state, handle, frozen) {
            Some(result) => value::encode_bool(result),
            None => fail_dispatch(ctx),
        };
    }
    let Ok(properties) = state.gc.heap().own_property_slots(handle) else {
        return fail_dispatch(ctx);
    };
    value::encode_bool(properties.into_iter().all(|(_, flags)| {
        flags & CONFIGURABLE == 0 && (!frozen || flags & ACCESSOR != 0 || flags & WRITABLE == 0)
    }))
}

/// TestIntegrityLevel 的数组部分（不可扩展性由调用方先验）：frozen 要求
/// length 不可写；全部覆盖层条目（下标与命名）须不可配置、数据条目在
/// frozen 下还须不可写；元素存储中每个非 hole 元素必须有覆盖层特性条目
/// （无条目即缺省可写可配置）。
fn array_integrity_level(state: &NativeAgentState, handle: u32, frozen: bool) -> Option<bool> {
    if frozen && !state.array_fixed_length.contains(&handle) {
        return Some(false);
    }
    let mut overlaid = std::collections::HashSet::new();
    for ((owner, key), flags) in &state.array_property_flags {
        if *owner != handle {
            continue;
        }
        if *flags & CONFIGURABLE != 0 || (frozen && *flags & WRITABLE != 0) {
            return Some(false);
        }
        if let Some(index) = overlay_element_index(state, *key) {
            overlaid.insert(index);
        }
    }
    for ((owner, key), (_, _, flags)) in &state.array_accessors {
        if *owner != handle {
            continue;
        }
        if *flags & CONFIGURABLE != 0 {
            return Some(false);
        }
        if let Some(index) = overlay_element_index(state, *key) {
            overlaid.insert(index);
        }
    }
    let length = state.gc.heap().array_length(handle).ok()?;
    for index in 0..length {
        let element = match state.gc.heap().get_element(handle, index) {
            Ok(Some(element)) => element as i64,
            Ok(None) => continue,
            Err(_) => return None,
        };
        if !value::is_array_hole(element) && !overlaid.contains(&index) {
            return Some(false);
        }
    }
    Some(true)
}

/// 覆盖层键还原为数组下标（非下标命名键返回 None）。
fn overlay_element_index(state: &NativeAgentState, key: PropertyKey) -> Option<u32> {
    super::runtime::array_index(state, super::runtime::encoded_property_key(key))
}
