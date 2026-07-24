use anyhow::Result;
use wasmtime::{Caller, Linker, Val};
use wjsm_ir::value;

use crate::RuntimeState;

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
    linker.func_wrap_async(
        "env",
        "gc_obj_get",
        |mut caller: Caller<'_, RuntimeState>, (object, key): (i64, i32)| {
            Box::new(async move {
                // V2 support obj_get 透传所有接收者；原始值必须在 host 侧分派，
                // 否则 `"".length` / Number.prototype 方法等全部变成 undefined。
                if value::is_string(object) {
                    return Ok(crate::host_imports::primitive_string_get_property_impl(
                        &mut caller,
                        object,
                        key as u32,
                    ));
                }
                if value::is_native_callable(object) {
                    return Ok(crate::runtime_linker::native_callable_get_property_impl(
                        &mut caller,
                        object,
                        key,
                    ));
                }
                if value::is_proxy(object) {
                    return Ok(
                        crate::host_imports::reentrant_async::proxy_trap_internal_get_async(
                            &mut caller,
                            object,
                            key,
                        )
                        .await,
                    );
                }
                // 与 V1 support obj_get 同序的原始值 tag 分派：undefined/null、
                // bigint、symbol、regexp、raw f64 均不携带 V2 heap handle。
                if value::is_undefined(object) || value::is_null(object) {
                    return Ok(value::encode_undefined());
                }
                if value::is_bigint(object) {
                    return Ok(crate::host_imports::primitive_bigint_get_method_impl(
                        &mut caller,
                        object,
                        key as u32,
                    ));
                }
                if value::is_symbol(object) {
                    return Ok(crate::runtime_heap::primitive_symbol_get_property_impl(
                        &mut caller,
                        object,
                        key as u32,
                    ));
                }
                if value::is_regexp(object) {
                    return Ok(crate::runtime_regexp::primitive_regexp_get_property_impl(
                        &mut caller,
                        object,
                        key as u32,
                    ));
                }
                if (object as u64 & value::BOX_BASE) != value::BOX_BASE {
                    return Ok(crate::host_imports::primitive_number_get_method_impl(
                        &mut caller,
                        object,
                        key as u32,
                    ));
                }
                let raw_key = key;
                // Function/closure/bound 的 own 属性在 function_props 对象上；
                // 先查 V2 handle table，未命中再回退 call/apply/bind 等内建解析。
                let handle = if value::is_function(object)
                    || value::is_closure(object)
                    || value::is_bound(object)
                {
                    crate::handle_index_of(&mut caller, object) as u32
                } else {
                    value::decode_handle(object)
                };
                if caller
                    .data()
                    .heap_access_v2()
                    .resolve_handle(handle)
                    .is_ok()
                {
                // heap-backed receiver 解析完成 = 一次 load barrier fast-path 事件。
                caller.data().count_barrier_load();
                    let key = property_key(&mut caller, key)?;
                    if value::is_array(object) {
                        let length_key = crate::property_key::encode_runtime_string_name_id(
                            crate::property_key::intern_runtime_property_key(
                                caller.data(),
                                crate::runtime_string::RuntimeString::from_utf8_str("length"),
                            ),
                        );
                        if key == length_key {
                            let length = caller.data().heap_access_v2().array_length(handle)?;
                            return Ok(value::encode_f64(length as f64));
                        }
                        // 数组命名属性（含 symbol）→ 宿主侧表；未命中落入原型链
                        // 解析 Array.prototype 方法（proto 走查会跳过数组 own 槽）。
                        if let Some(slot) =
                            crate::array_named_props::ArrayNamedPropsStore::get_slot(
                                &mut caller,
                                object,
                                key,
                            )
                        {
                            return Ok(slot.value);
                        }
                    }
                    let access = caller.data().heap_access_v2().clone();
                    match access.get_property_slot_on_proto_chain(handle, key) {
                        Ok(property) => {
                            if property.is_some()
                                || !(value::is_function(object)
                                    || value::is_closure(object)
                                    || value::is_bound(object))
                            {
                                return read_v2_property_async(&mut caller, object, property)
                                    .await;
                            }
                            return Ok(
                                crate::runtime_linker::function_value_get_property_impl(
                                    &mut caller,
                                    object,
                                    raw_key,
                                ),
                            );
                        }
                        Err(crate::runtime_gc::HeapAccessV2Error::ProxyPrototype {
                            handle: proto_handle,
                        }) => {
                            // 原型链上的 Proxy：用 proxy 值继续 [[Get]]。
                            let proxy = value::encode_proxy_handle(proto_handle & 0x7FFF_FFFF);
                            return Ok(
                                crate::host_imports::reentrant_async::proxy_trap_internal_get_async(
                                    &mut caller,
                                    proxy,
                                    key as i32,
                                )
                                .await,
                            );
                        }
                        Err(error) => return Err(host_error(error)),
                    }
                } else if value::is_function(object)
                    || value::is_closure(object)
                    || value::is_bound(object)
                {
                    return Ok(crate::runtime_linker::function_value_get_property_impl(
                        &mut caller,
                        object,
                        raw_key,
                    ));
                }
                Ok(value::encode_undefined())
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "gc_obj_set",
        |mut caller: Caller<'_, RuntimeState>, (object, key, new_value): (i64, i32, i64)| {
            Box::new(async move {
                let raw_key = key as u32;
                let key = property_key(&mut caller, key)?;
                if value::is_proxy(object) {
                    set_proxy_property_v2(&mut caller, object, raw_key, key, new_value)?;
                    return Ok(());
                }
                // TAG_REGEXP：lastIndex 等属性由 regexp 专用 owner 承载
                // （与 V1 support obj_set 的 TAG_REGEXP 分派一致）。
                if value::is_regexp(object) {
                    crate::runtime_regexp::primitive_regexp_set_property_impl(
                        &mut caller,
                        object,
                        raw_key,
                        new_value,
                    );
                    return Ok(());
                }
                // Function/closure/bound 的属性对象 handle 从 function_props_base 起算。
                let handle = if value::is_function(object)
                    || value::is_closure(object)
                    || value::is_bound(object)
                {
                    crate::handle_index_of(&mut caller, object) as u32
                } else {
                    value::decode_handle(object)
                };
                // heap receiver 写路径（proxy/regexp 已提前分派）= store barrier fast-path 事件。
                caller.data().count_barrier_store();
                if value::is_array(object) {
                    let length_key = crate::property_key::encode_runtime_string_name_id(
                        crate::property_key::intern_runtime_property_key(
                            caller.data(),
                            crate::runtime_string::RuntimeString::from_utf8_str("length"),
                        ),
                    );
                    if key == length_key {
                        crate::host_imports::array_set_length_impl(&mut caller, object, new_value);
                        return Ok(());
                    }
                    // 数组命名属性（元素走 elem_set）：own slot 尊重 writable，
                    // 缺失时按可扩展性新建——与 V1 support obj_set 数组分支同语义。
                    if let Some(slot) = crate::array_named_props::ArrayNamedPropsStore::get_slot(
                        &mut caller,
                        object,
                        key,
                    ) {
                        if slot.flags & wjsm_ir::constants::FLAG_WRITABLE == 0 {
                            return Ok(());
                        }
                        crate::array_named_props::ArrayNamedPropsStore::set_with_flags(
                            &mut caller,
                            object,
                            key,
                            new_value,
                            slot.flags,
                        );
                        return Ok(());
                    }
                    if !crate::is_extensible_impl(&mut caller, object) {
                        return Ok(());
                    }
                    crate::array_named_props::ArrayNamedPropsStore::set(
                        &mut caller,
                        object,
                        key,
                        new_value,
                    );
                    return Ok(());
                }
                // eval 编译的函数从未执行 __wjsm_init_function_props，
                // 其 handle 在 V2 handle table 中为空；按需分配属性对象。
                let access = caller.data().heap_access_v2().clone();
                if access.resolve_handle(handle).is_err()
                    && (value::is_function(object)
                        || value::is_closure(object)
                        || value::is_bound(object))
                {
                    let proto_handle =
                        value::decode_object_handle(caller.data().function_prototype);
                    let capacity = 4u32;
                    let bytes = u64::from(capacity)
                        * u64::from(wjsm_ir::constants::HEAP_OBJECT_PROPERTY_SLOT_SIZE)
                        + u64::from(wjsm_ir::constants::HEAP_OBJECT_HEADER_SIZE);
                    if let Ok((object_addr, _)) =
                        crate::runtime_heap::allocate_v2_object_bytes_with_context(
                            &mut caller,
                            bytes,
                        )
                    {
                        let _ = access.publish_object(handle, object_addr, proto_handle, capacity);
                    }
                }
                // OrdinarySet：accessor 调 setter；own 数据写值；缺失时在 receiver 新建。
                if let Some(property) = access
                    .get_property_slot_on_proto_chain(handle, key)
                    .map_err(host_error)?
                {
                    if property.flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32 != 0 {
                        let setter = property.setter as i64;
                        if value::is_undefined(setter) || value::is_null(setter) {
                            return Ok(());
                        }
                        if value::is_callable(setter) {
                            let _ = crate::runtime_host_helpers::call_wasm_callback_async(
                                &mut caller,
                                setter,
                                object,
                                &[new_value],
                            )
                            .await
                            .map_err(host_error)?;
                        }
                        return Ok(());
                    }
                    let own = access
                        .get_property_slot(handle, key)
                        .map_err(host_error)?
                        .is_some();
                    if own {
                        if property.flags & wjsm_ir::constants::FLAG_WRITABLE as u32 == 0 {
                            return Ok(());
                        }
                        access
                            .set_property(handle, key, new_value as u64)
                            .map_err(host_error)?;
                        return Ok(());
                    }
                    // proto 数据属性：在 receiver 上 CreateDataProperty（可写）或拒绝（只读）。
                    if property.flags & wjsm_ir::constants::FLAG_WRITABLE as u32 == 0 {
                        return Ok(());
                    }
                }
                if !crate::is_extensible_impl(&mut caller, object) {
                    return Ok(());
                }
                access
                    .set_property(handle, key, new_value as u64)
                    .map_err(host_error)?;
                Ok(())
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "gc_obj_delete",
        |mut caller: Caller<'_, RuntimeState>, (object, key): (i64, i32)| {
            Box::new(async move {
                if value::is_proxy(object) {
                    return Ok(
                        crate::host_imports::reentrant_async::proxy_trap_internal_delete_async(
                            &mut caller,
                            object,
                            key,
                        )
                        .await,
                    );
                }
                let handle = value::decode_handle(object);
                let key = property_key(&mut caller, key)?;
                if value::is_array(object) {
                    if let Some(name) = match crate::property_key::decode_name_id(key) {
                        crate::property_key::DecodedNameId::RuntimeString(index) => {
                            crate::property_key::runtime_property_key_units(caller.data(), index)
                                .map(|name| name.to_utf8_lossy())
                        }
                        crate::property_key::DecodedNameId::MemoryString(index) => {
                            let env = crate::WasmEnv::from_caller(&mut caller)
                                .ok_or_else(|| wasmtime::Error::msg("missing WasmEnv"))?;
                            let bytes = crate::runtime_render::read_string_bytes_mem(
                                &caller,
                                &env.memory,
                                index,
                            );
                            Some(String::from_utf8_lossy(&bytes).into_owned())
                        }
                        crate::property_key::DecodedNameId::Symbol(_) => None,
                    } && let Ok(index) = name.parse::<u32>()
                        && let Some(ptr) = crate::resolve_array_ptr(&mut caller, object)
                    {
                        crate::runtime_values::write_array_hole(&mut caller, ptr, index);
                        return Ok(value::encode_bool(true));
                    }
                    // 非索引命名属性（含 symbol）→ 宿主侧表；
                    // configurable=false → false，不存在 → true。
                    return Ok(value::encode_bool(
                        crate::array_named_props::ArrayNamedPropsStore::remove(
                            &mut caller,
                            object,
                            key,
                        )
                        .unwrap_or(true),
                    ));
                }
                let access = caller.data().heap_access_v2().clone();
                if let Some(property) = access.get_property_slot(handle, key).map_err(host_error)? {
                    if property.flags & wjsm_ir::constants::FLAG_CONFIGURABLE as u32 == 0 {
                        return Ok(value::encode_bool(false));
                    }
                    let deleted = access.delete_property(handle, key)?;
                    return Ok(value::encode_bool(deleted));
                }
                Ok(value::encode_bool(true))
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
                // TypedArray 数字索引先走 Rust 侧 typedarray 表
                // （与 V1 obj_get_by_index 分派一致；负数索引落入属性路径 → undefined）。
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
                // heap 中介的元素读（TypedArray Rust 表已提前分派）= load barrier 事件。
                caller.data().count_barrier_load();
                // arguments 等对象以 "0"/"1" 属性键承载索引访问，非数组布局。
                if !value::is_array(array)
                    && access.object_type(handle).ok() != Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY))
                {
                    let key = v2_index_property_key(&caller, index);
                    let property = access.get_property_slot_on_proto_chain(handle, key)?;
                    return read_v2_property_async(&mut caller, array, property).await;
                }
                let index = u32::try_from(index).map_err(host_error)?;
                Ok(access
                    .get_element(handle, index)?
                    .unwrap_or(value::encode_undefined() as u64) as i64)
            })
        },
    )?;
    linker.func_wrap(
        "env",
        "gc_elem_set",
        |mut caller: Caller<'_, RuntimeState>,
         array: i64,
         index: i32,
         new_value: i64|
         -> wasmtime::Result<()> {
            // TypedArray 数字索引写入 Rust 侧表（负数索引按规范丢弃）。
            if crate::runtime_typedarray::typedarray_entry_from_value(&mut caller, array).is_some()
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
            // heap 中介的元素写（TypedArray Rust 表已提前分派）= store barrier 事件。
            caller.data().count_barrier_store();
            if !value::is_array(array)
                && access.object_type(handle).ok() != Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY))
            {
                let key = v2_index_property_key(&caller, index);
                access.set_property(handle, key, new_value as u64)?;
                return Ok(());
            }
            let index = u32::try_from(index).map_err(host_error)?;
            crate::set_v2_array_element(&mut caller, handle, index, new_value as u64)?;
            Ok(())
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
fn property_key(caller: &mut Caller<'_, RuntimeState>, key: i32) -> wasmtime::Result<u32> {
    crate::property_key::canonicalize_v2_name_id(caller, key as u32).ok_or_else(|| {
        wasmtime::Error::msg(format!(
            "V2 property key offset {} is outside main memory",
            key as u32
        ))
    })
}

async fn read_v2_property_async(
    caller: &mut Caller<'_, RuntimeState>,
    receiver: i64,
    property: Option<crate::runtime_gc::HeapAccessV2Property>,
) -> wasmtime::Result<i64> {
    let Some(property) = property else {
        return Ok(value::encode_undefined());
    };
    if property.flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32 == 0 {
        return Ok(property.value as i64);
    }
    if value::is_undefined(property.getter as i64) {
        return Ok(value::encode_undefined());
    }
    crate::runtime_host_helpers::call_wasm_callback_async(
        caller,
        property.getter as i64,
        receiver,
        &[],
    )
    .await
    .map_err(host_error)
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

fn set_proxy_property_v2(
    caller: &mut Caller<'_, RuntimeState>,
    proxy: i64,
    raw_key: u32,
    key: u32,
    new_value: i64,
) -> wasmtime::Result<()> {
    let handle = value::decode_proxy_handle(proxy) as usize;
    let entry = caller
        .data()
        .proxy_table
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(handle)
        .cloned()
        .ok_or_else(|| wasmtime::Error::msg("invalid V2 proxy handle"))?;
    if entry.revoked {
        return Err(wasmtime::Error::msg(
            "TypeError: Cannot perform 'set' on a proxy that has been revoked",
        ));
    }
    let trap = crate::runtime_heap::read_host_data_property_v2(caller, entry.handler, "set")
        .unwrap_or_else(value::encode_undefined);
    if value::is_undefined(trap) || value::is_null(trap) {
        return set_proxy_target_property_v2(caller, entry.target, raw_key, key, new_value);
    }
    let prop = crate::property_key::name_id_to_property_key_value(raw_key)
        .ok_or_else(|| wasmtime::Error::msg("invalid V2 proxy property key"))?;
    let runtime = tokio::runtime::Handle::current();
    tokio::task::block_in_place(|| {
        runtime.block_on(crate::runtime_host_helpers::call_wasm_callback_async(
            caller,
            trap,
            entry.handler,
            &[entry.target, prop, new_value, proxy],
        ))
    })
    .map_err(host_error)?;
    Ok(())
}

fn set_proxy_target_property_v2(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    raw_key: u32,
    key: u32,
    new_value: i64,
) -> wasmtime::Result<()> {
    if value::is_proxy(target) {
        return set_proxy_property_v2(caller, target, raw_key, key, new_value);
    }
    // 数组 target：length / 命名属性与 gc_obj_set 数组分支同语义。
    if value::is_array(target) {
        let length_key = crate::property_key::encode_runtime_string_name_id(
            crate::property_key::intern_runtime_property_key(
                caller.data(),
                crate::runtime_string::RuntimeString::from_utf8_str("length"),
            ),
        );
        if key == length_key {
            crate::host_imports::array_set_length_impl(caller, target, new_value);
            return Ok(());
        }
        if let Some(slot) =
            crate::array_named_props::ArrayNamedPropsStore::get_slot(caller, target, key)
        {
            if slot.flags & wjsm_ir::constants::FLAG_WRITABLE == 0 {
                return Ok(());
            }
            crate::array_named_props::ArrayNamedPropsStore::set_with_flags(
                caller, target, key, new_value, slot.flags,
            );
            return Ok(());
        }
        crate::array_named_props::ArrayNamedPropsStore::set(caller, target, key, new_value);
        return Ok(());
    }
    if !value::is_object(target) {
        return Err(wasmtime::Error::msg(
            "TypeError: Proxy target is not an object",
        ));
    }
    caller
        .data()
        .heap_access_v2()
        .set_property(value::decode_handle(target), key, new_value as u64)
        .map_err(host_error)
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
