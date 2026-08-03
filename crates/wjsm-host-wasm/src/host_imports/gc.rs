use anyhow::Result;
use wasmtime::{Caller, Linker, Val};
use wjsm_ir::value;

use crate::RuntimeState;

/// `gc_obj_get_data`/`gc_obj_set_data` 快路径的「需走慢路径」哨兵。
///
/// tag 0x1E 不在运行时任何编码路径中产生（有效 tag 为 0x0..=0x12），因此该值
/// 不可能成为真实属性值；support 层 obj_get/obj_set 仅用它做内部协议判断，
/// 命中后回退 async `gc_obj_get`/`gc_obj_set` 完整语义（accessor getter/setter、
/// proxy trap、非普通对象、未命中/创建等）。
const FAST_PATH_MISS: i64 = (value::BOX_BASE as i64) | (0x1E_i64 << 32);

/// 快路径链查找的最大深度：超出即回退慢路径（防御异常原型链环）。
const FAST_PATH_MAX_DEPTH: u32 = 32;

/// 同步数据槽读取快路径：仅当 `object` 是普通对象（TAG_OBJECT）且属性在原型链上
/// 以**数据槽**形态存在时直接返回槽值（闭包 env 变量读取的典型形态），否则返回
/// `FAST_PATH_MISS`。语义与 `get_by_name_id` 的 `lookup_property_on_proto` 分支
/// 完全一致：同一 canonicalize + `get_property_slot_on_proto_chain`，仅省去 async
/// 调用机制与 accessor 派发。
fn gc_obj_get_data_impl(caller: &mut Caller<'_, RuntimeState>, object: i64, key: i32) -> i64 {
    if !value::is_object(object) {
        return FAST_PATH_MISS;
    }
    let Some(key) = crate::property_key::canonicalize_v2_name_id(caller, key as u32) else {
        return FAST_PATH_MISS;
    };
    // 已确认 is_object：handle 即低 32 位（handle_index_of 对普通对象直接取低 32 位）。
    let handle = (object as u64 & 0xFFFF_FFFF) as u32;
    let access = caller.data().heap_access_v2().clone();
    match access.get_property_slot_on_proto_chain(handle, key) {
        Ok(Some(slot)) if slot.flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32 == 0 => {
            // 槽值按位解释为 JS 值（NaN-boxed i64）。
            slot.value as i64
        }
        _ => FAST_PATH_MISS,
    }
}

/// 同步数据槽写入快路径：覆盖「就地写」与「在普通对象上定义新数据槽」两种形态，
/// 与 async `ordinary_set_by_name_id` 逐分支对齐：
/// - receiver 自身存在可写数据槽 → 就地写（闭包 env 变量更新的典型形态）；
/// - 原型链上存在 accessor / 非可写数据槽 / 数组 / proxy → 回退慢路径
///   （setter 调用 / 严格模式失败 / ArrayNamedPropsStore 语义）；
/// - 链上未命中或命中可写数据槽（shadow）→ receiver 可扩展时定义数据槽
///   （闭包 env 创建时首个 SetProp 的典型形态），不可扩展回退慢路径。
fn gc_obj_set_data_impl(
    caller: &mut Caller<'_, RuntimeState>,
    object: i64,
    key: i32,
    value: i64,
) -> i64 {
    if !value::is_object(object) {
        return FAST_PATH_MISS;
    }
    let Some(key) = crate::property_key::canonicalize_v2_name_id(caller, key as u32) else {
        return FAST_PATH_MISS;
    };
    // 已确认 is_object：handle 即低 32 位（handle_index_of 对普通对象直接取低 32 位）。
    let handle = (object as u64 & 0xFFFF_FFFF) as u32;
    let access = caller.data().heap_access_v2().clone();
    // 1) 自身已有槽：数据 + 可写 → 就地写；否则回退。
    if let Ok(Some(slot)) = access.get_property_slot(handle, key) {
        if slot.flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32 != 0
            || slot.flags & wjsm_ir::constants::FLAG_WRITABLE as u32 == 0
        {
            return FAST_PATH_MISS;
        }
        return match access.set_property(handle, key, value as u64) {
            // value 按位解释为槽值（NaN-boxed i64 → u64）。
            Ok(()) => value::encode_undefined(),
            Err(_) => FAST_PATH_MISS,
        };
    }
    // 2) 链查找：accessor / 非可写 / 数组 / proxy → 回退；
    //    可写数据槽（外层）或未命中 → 在 receiver 上定义（shadow/创建）。
    let mut current = handle;
    for _ in 0..FAST_PATH_MAX_DEPTH {
        match access.get_property_slot(current, key) {
            Ok(Some(slot)) => {
                if slot.flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32 != 0
                    || slot.flags & wjsm_ir::constants::FLAG_WRITABLE as u32 == 0
                {
                    return FAST_PATH_MISS;
                }
                break; // 外层可写数据槽 → shadow 定义
            }
            Ok(None) => {
                // 数组在链上拥有命名属性（ArrayNamedPropsStore）→ 回退
                if access.object_type(current).ok() == Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY)) {
                    return FAST_PATH_MISS;
                }
                let Ok(proto) = access.prototype(current) else {
                    return FAST_PATH_MISS;
                };
                if proto == 0xFFFF_FFFF || proto == current || proto & 0x8000_0000 != 0 {
                    break; // 链尽头 → 创建定义
                }
                current = proto;
            }
            Err(_) => return FAST_PATH_MISS,
        }
    }
    // 与 define_value_on_receiver 一致：不可扩展 → 回退慢路径（严格模式抛错语义）。
    if !crate::is_extensible_impl(caller, object) {
        return FAST_PATH_MISS;
    }
    match access.set_property(handle, key, value as u64) {
        // value 按位解释为槽值（NaN-boxed i64 → u64）；未命中时以默认
        // 数据属性 flags（configurable|enumerable|writable）定义新槽。
        Ok(()) => value::encode_undefined(),
        Err(_) => FAST_PATH_MISS,
    }
}

pub(crate) fn allocate_v2_array_handle(
    caller: &mut Caller<'_, RuntimeState>,
    capacity: u32,
) -> wasmtime::Result<u32> {
    let prototype = ensure_v2_array_prototype(caller)?;
    let handle = take_next_handle(caller)?;
    let bytes = u64::from(capacity)
        .checked_mul(8)
        .and_then(|elements| {
            elements.checked_add(wjsm_ir::constants::HEAP_OBJECT_HEADER_SIZE as u64)
        })
        .ok_or_else(|| wasmtime::Error::msg("V2 array size overflow"))?;
    let access = caller.data().heap_access_v2().clone();
    let (object, _) = crate::allocate_v2_object_bytes(caller, bytes)?;
    access.publish_array(handle, object, prototype, capacity)?;
    Ok(handle)
}

pub(crate) fn define_v2(linker: &mut Linker<RuntimeState>) -> Result<()> {
    linker.func_wrap(
        "env",
        "gc_alloc_slow",
        |mut caller: Caller<'_, RuntimeState>,
         bytes: i64,
         _heap_type: i32,
         _capacity: i32|
         -> wasmtime::Result<i64> {
            let bytes = u64::try_from(bytes).map_err(host_error)?;
            let (start, _) = crate::allocate_v2_object_bytes(&mut caller, bytes)?;
            Ok(start as i64)
        },
    )?;
    // 同步数据槽读写快路径（support 模块 obj_get/obj_set 优先调用；
    // 命中 FAST_PATH_MISS 哨兵时回退下方 async 版本）。
    linker.func_wrap(
        "env",
        "gc_obj_get_data",
        |mut caller: Caller<'_, RuntimeState>, object: i64, key: i32| -> i64 {
            gc_obj_get_data_impl(&mut caller, object, key)
        },
    )?;
    linker.func_wrap(
        "env",
        "gc_obj_set_data",
        |mut caller: Caller<'_, RuntimeState>, object: i64, key: i32, new_value: i64| -> i64 {
            gc_obj_set_data_impl(&mut caller, object, key, new_value)
        },
    )?;
    linker.func_wrap_async(
        "env",
        "gc_obj_get",
        |mut caller: Caller<'_, RuntimeState>, (object, key): (i64, i32)| {
            Box::new(async move {
                let name_id = key as u32;
                if value::is_js_object(object) && !value::is_proxy(object) {
                    caller.data().count_barrier_load();
                }
                Ok(wjsm_builtins::property::get_by_name_id(
                    &mut crate::exec_context_impl::WasmExecContext::new(&mut caller),
                    object,
                    name_id,
                )
                .await)
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "gc_obj_set",
        |mut caller: Caller<'_, RuntimeState>, (object, key, new_value): (i64, i32, i64)| {
            Box::new(async move {
                let name_id = key as u32;
                if value::is_js_object(object)
                    && !value::is_proxy(object)
                    && !value::is_regexp(object)
                {
                    caller.data().count_barrier_store();
                }
                wjsm_builtins::property::set_by_name_id(
                    &mut crate::exec_context_impl::WasmExecContext::new(&mut caller),
                    object,
                    name_id,
                    new_value,
                )
                .await;
                Ok(())
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "gc_obj_delete",
        |mut caller: Caller<'_, RuntimeState>, (object, key): (i64, i32)| {
            Box::new(async move {
                let name_id = key as u32;
                Ok(wjsm_builtins::property::delete_by_name_id(
                    &mut crate::exec_context_impl::WasmExecContext::new(&mut caller),
                    object,
                    name_id,
                )
                .await)
            })
        },
    )?;
    linker.func_wrap(
        "env",
        "gc_arr_new",
        |mut caller: Caller<'_, RuntimeState>, capacity: i32| -> wasmtime::Result<i32> {
            let capacity = u32::try_from(capacity).map_err(host_error)?;
            Ok(allocate_v2_array_handle(&mut caller, capacity)? as i32)
        },
    )?;
    linker.func_wrap_async(
        "env",
        "gc_elem_get",
        |mut caller: Caller<'_, RuntimeState>, (array, index): (i64, i32)| {
            Box::new(async move {
                if index >= 0
                    && let Some(element) = crate::runtime_typedarray::typedarray_element_read(
                        &mut caller,
                        array,
                        index as u32,
                    )
                {
                    return Ok(element);
                }
                let handle = value::decode_handle(array);
                let access = caller.data().heap_access_v2().clone();
                caller.data().count_barrier_load();
                if value::is_array(array)
                    && index >= 0
                    && access.object_type(handle).ok()
                        == Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY))
                {
                    return Ok(access
                        .get_element(handle, index as u32)?
                        .unwrap_or(value::encode_undefined() as u64)
                        as i64);
                }
                let name_id = v2_index_property_key(&caller, index);
                Ok(wjsm_builtins::property::get_by_name_id(
                    &mut crate::exec_context_impl::WasmExecContext::new(&mut caller),
                    array,
                    name_id,
                )
                .await)
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "gc_elem_set",
        |mut caller: Caller<'_, RuntimeState>, (array, index, new_value): (i64, i32, i64)| {
            Box::new(async move {
                if crate::runtime_typedarray::typedarray_entry_from_value(&mut caller, array)
                    .is_some()
                {
                    if index >= 0 {
                        let _ = crate::runtime_typedarray::typedarray_element_write(
                            &mut caller,
                            array,
                            index as u32,
                            new_value,
                        );
                    }
                    return Ok(());
                }
                let handle = value::decode_handle(array);
                let access = caller.data().heap_access_v2().clone();
                caller.data().count_barrier_store();
                if value::is_array(array)
                    && index >= 0
                    && access.object_type(handle).ok()
                        == Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY))
                {
                    crate::set_v2_array_element(
                        &mut caller,
                        handle,
                        index as u32,
                        new_value as u64,
                    )?;
                    return Ok(());
                }
                let name_id = v2_index_property_key(&caller, index);
                wjsm_builtins::property::set_by_name_id(
                    &mut crate::exec_context_impl::WasmExecContext::new(&mut caller),
                    array,
                    name_id,
                    new_value,
                )
                .await;
                Ok(())
            })
        },
    )?;
    Ok(())
}

/// 数值下标 → 规范化 V2 属性键（与 define_host_data_property_v2 同一 intern 表）。
fn v2_index_property_key(caller: &Caller<'_, RuntimeState>, index: i32) -> u32 {
    crate::property_key::encode_runtime_string_name_id(
        crate::property_key::intern_runtime_property_key(
            caller.data(),
            crate::runtime_string::RuntimeString::from_utf8_str(&index.to_string()),
        ),
    )
}


fn ensure_v2_array_prototype(caller: &mut Caller<'_, RuntimeState>) -> wasmtime::Result<u32> {
    let env = crate::WasmEnv::from_caller(caller)
        .ok_or_else(|| wasmtime::Error::msg("missing cached WasmEnv"))?;
    let current =
        env.array_proto_handle
            .get(&mut *caller)
            .i32()
            .ok_or_else(|| wasmtime::Error::msg("__array_proto_handle is not i32"))? as u32;
    let values_key = crate::property_key::encode_runtime_string_name_id(
        crate::property_key::intern_runtime_property_key(
            caller.data(),
            crate::runtime_string::RuntimeString::from_utf8_str("values"),
        ),
    );
    if caller
        .data()
        .heap_access_v2()
        .get_property(current, values_key)
        .ok()
        .flatten()
        .is_some()
    {
        return Ok(current);
    }
    let methods =
        wjsm_backend_wasm::host_import_registry::array_proto_method_specs().collect::<Vec<_>>();
    let capacity = u32::try_from(methods.len() + 4)
        .map_err(|_| wasmtime::Error::msg("V2 Array.prototype method table is too large"))?;
    let prototype = crate::alloc_host_object_v2(caller, capacity);
    if !value::is_object(prototype) {
        return Err(wasmtime::Error::msg(
            "V2 Array.prototype allocation did not return an object",
        ));
    }
    let handle = value::decode_handle(prototype);
    env.array_proto_handle
        .set(&mut *caller, Val::I32(handle as i32))
        .map_err(host_error)?;
    let table_base = env
        .arr_proto_table_base
        .and_then(|global| global.get(&mut *caller).i32())
        .ok_or_else(|| wasmtime::Error::msg("missing __arr_proto_table_base global"))?
        as u32;
    for (offset, (_, spec)) in methods.into_iter().enumerate() {
        let name = wjsm_backend_wasm::host_import_registry::array_proto_property_name(spec.name)
            .ok_or_else(|| wasmtime::Error::msg("invalid Array.prototype method name"))?;
        let callable = value::encode_function_idx(table_base + offset as u32);
        if crate::define_host_data_property_from_caller(caller, prototype, &name, callable)
            .is_none()
        {
            return Err(wasmtime::Error::msg(
                "V2 Array.prototype method installation failed",
            ));
        }
    }
    crate::runtime_startup::install_array_proto_to_string(caller, &env, prototype)
        .ok_or_else(|| {
            wasmtime::Error::msg("V2 Array.prototype toString installation failed")
        })?;
    let iterator_value =
        crate::create_native_callable(caller.data(), crate::NativeCallable::ArrayProtoValues);
    let keys = crate::create_native_callable(caller.data(), crate::NativeCallable::ArrayProtoKeys);
    let entries =
        crate::create_native_callable(caller.data(), crate::NativeCallable::ArrayProtoEntries);
    if crate::define_host_data_property_from_caller(caller, prototype, "values", iterator_value)
        .is_none()
        || crate::define_host_data_property_from_caller(caller, prototype, "keys", keys).is_none()
        || crate::define_host_data_property_from_caller(caller, prototype, "entries", entries)
            .is_none()
    {
        return Err(wasmtime::Error::msg(
            "V2 Array.prototype iterator method installation failed",
        ));
    }
    if crate::define_host_data_property_by_name_id_with_flags(
        caller,
        prototype,
        crate::encode_symbol_name_id(wjsm_ir::wk_symbol::ITERATOR),
        iterator_value,
        wjsm_ir::constants::FLAG_CONFIGURABLE | wjsm_ir::constants::FLAG_WRITABLE,
    )
    .is_none()
    {
        return Err(wasmtime::Error::msg(
            "V2 Array.prototype iterator property installation failed",
        ));
    }
    Ok(handle)
}



fn take_next_handle(caller: &mut Caller<'_, RuntimeState>) -> wasmtime::Result<u32> {
    let env = crate::WasmEnv::from_caller(caller)
        .ok_or_else(|| wasmtime::Error::msg("missing cached WasmEnv"))?;
    let current = env
        .obj_table_count
        .get(&mut *caller)
        .i32()
        .ok_or_else(|| wasmtime::Error::msg("__obj_table_count is not i32"))?;
    let next = current
        .checked_add(1)
        .ok_or_else(|| wasmtime::Error::msg("V2 handle table exhausted"))?;
    env.obj_table_count
        .set(&mut *caller, Val::I32(next))
        .map_err(host_error)?;
    Ok(current as u32)
}

fn host_error(error: impl std::fmt::Display) -> wasmtime::Error {
    wasmtime::Error::msg(error.to_string())
}
