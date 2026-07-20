use super::*;

pub(crate) fn arguments_strict_callee_getter(
    caller: &mut Caller<'_, RuntimeState>,
    _this: i64,
) -> i64 {
    make_type_error_exception(
        caller,
        "TypeError: 'callee' and 'caller' properties are not defined",
    )
}

const ARGUMENTS_DATA_FLAGS: i32 = constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE;

fn define_arguments_data_property(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    val: i64,
) -> Option<()> {
    let name_id =
        find_memory_c_string(caller, name).or_else(|| alloc_heap_c_string(caller, name))?;
    define_host_data_property_by_name_id_with_flags(
        caller,
        obj,
        encode_string_name_id(name_id),
        val,
        ARGUMENTS_DATA_FLAGS,
    )
}

fn get_array_proto_values(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let cached = caller.data().array_proto_values.load(Ordering::Relaxed);
    if !value::is_undefined(cached) {
        return cached;
    }
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let handle = env.array_proto_handle.get(&mut *caller).i32().unwrap_or(-1);
    if handle < 0 {
        return value::encode_undefined();
    }
    let array_proto_obj = value::encode_object_handle(handle as u32);
    let mut values = {
        #[cfg(feature = "managed-heap-v2")]
        {
            let access = caller.data().heap_access_v2().clone();
            if access.resolve_handle(handle as u32).is_ok() {
                let key = crate::property_key::encode_runtime_string_name_id(
                    crate::property_key::intern_runtime_property_key(
                        caller.data(),
                        crate::runtime_string::RuntimeString::from_utf8_str("values"),
                    ),
                );
                access
                    .get_property(handle as u32, key)
                    .ok()
                    .flatten()
                    .map(|value| value as i64)
                    .unwrap_or_else(value::encode_undefined)
            } else {
                value::encode_undefined()
            }
        }
        #[cfg(not(feature = "managed-heap-v2"))]
        {
            resolve_handle_idx_with_env(caller, &env, handle as usize)
                .and_then(|ptr| read_object_property_by_name_with_env(caller, &env, ptr, "values"))
                .unwrap_or_else(value::encode_undefined)
        }
    };
    if value::is_undefined(values) {
        values = create_native_callable(caller.data(), NativeCallable::ArrayProtoValues);
        let _ = define_arguments_data_property(caller, array_proto_obj, "values", values);
    }
    let _ = define_host_data_property_by_name_id_with_flags(
        caller,
        array_proto_obj,
        encode_symbol_name_id(wjsm_ir::wk_symbol::ITERATOR),
        values,
        ARGUMENTS_DATA_FLAGS,
    );
    caller
        .data()
        .array_proto_values
        .store(values, Ordering::Relaxed);
    values
}

fn define_arguments_iterator_property(caller: &mut Caller<'_, RuntimeState>, obj: i64) {
    let array_values = get_array_proto_values(caller);
    let _ = define_host_data_property_by_name_id_with_flags(
        caller,
        obj,
        encode_symbol_name_id(wjsm_ir::wk_symbol::ITERATOR),
        array_values,
        ARGUMENTS_DATA_FLAGS,
    );
}
/// 覆写 heap type 为 HEAP_TYPE_ARGUMENTS 用于 [object Arguments] 检测。
/// V2 下 resolve_handle 返回 handle id 而非线性内存指针，必须走 HeapAccessV2 owner。
fn override_arguments_heap_type(caller: &mut Caller<'_, RuntimeState>, obj: i64) {
    #[cfg(feature = "managed-heap-v2")]
    {
        let handle = value::decode_handle(obj);
        let access = caller.data().heap_access_v2().clone();
        if access.resolve_handle(handle).is_ok() {
            let _ = access.set_object_type(handle, wjsm_ir::HEAP_TYPE_ARGUMENTS);
            return;
        }
    }
    if let Some(ptr) = resolve_handle(caller, obj)
        && let Some(Extern::Memory(mem)) = caller.get_export("memory")
    {
        let data = mem.data_mut(&mut *caller);
        if ptr + 4 < data.len() {
            data[ptr + 4] = wjsm_ir::HEAP_TYPE_ARGUMENTS;
        }
    }
}

/// CreateUnmappedArgumentsObject (ES §10.4.4.6)
///
/// 用于严格模式函数、箭头函数、方法、类字段。
/// 无 [[ParameterMap]] —— 只有索引属性 + length 的普通对象。
pub(crate) fn create_unmapped_arguments_object(
    caller: &mut Caller<'_, RuntimeState>,
    args_array: i64,
    param_count: i64,
) -> i64 {
    let _param_count = value::decode_f64(param_count) as u32;

    // 先计算实际参数个数，确定 capacity（索引属性 + length）
    let arr_ptr = if value::is_array(args_array) {
        resolve_handle(caller, args_array)
    } else {
        None
    };
    let len = arr_ptr
        .and_then(|ptr| read_array_length(caller, ptr))
        .unwrap_or(0);
    let capacity = (len + 3).max(4);
    let obj = {
        let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_host_object(caller, &_wjsm_env, capacity)
    };

    override_arguments_heap_type(caller, obj);

    // 复制参数值作为索引属性
    if let Some(ptr) = arr_ptr {
        for i in 0..len as usize {
            let val = read_array_elem(caller, ptr, i as u32).unwrap_or(value::encode_undefined());
            let _ = define_host_data_property_from_caller(caller, obj, &i.to_string(), val);
        }
    }

    // Set length = 实际参数个数（writable, enumerable=false, configurable=true）
    let _ = define_arguments_data_property(caller, obj, "length", value::encode_f64(len as f64));
    define_arguments_iterator_property(caller, obj);

    let callee_getter = {
        let mut table = caller
            .data()
            .native_callables
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(crate::NativeCallable::ArgumentsStrictCalleeGetter);
        value::encode_native_callable_idx(handle)
    };
    let _ = define_host_accessor_property_with_flags(
        caller,
        obj,
        "callee",
        callee_getter,
        callee_getter,
        0,
    );

    obj
}

/// CreateMappedArgumentsObject (ES §10.4.4.7)
///
/// 用于非严格模式、非箭头、非方法、非类的函数。
/// 有 [[ParameterMap]] 实现双向绑定，以及 callee。
pub(crate) fn create_mapped_arguments_object(
    caller: &mut Caller<'_, RuntimeState>,
    args_array: i64,
    param_count: i64,
    func_ref: i64,
) -> i64 {
    let _param_count = value::decode_f64(param_count) as u32;

    // 先计算实际参数个数，确定 capacity（索引属性 + length + callee）
    let arr_ptr = if value::is_array(args_array) {
        resolve_handle(caller, args_array)
    } else {
        None
    };
    let len = arr_ptr
        .and_then(|ptr| read_array_length(caller, ptr))
        .unwrap_or(0);
    let capacity = (len + 3).max(4);
    let obj = {
        let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_host_object(caller, &_wjsm_env, capacity)
    };

    override_arguments_heap_type(caller, obj);

    // 复制参数值作为索引属性
    if let Some(ptr) = arr_ptr {
        for i in 0..len as usize {
            let val = read_array_elem(caller, ptr, i as u32).unwrap_or(value::encode_undefined());
            let _ = define_host_data_property_from_caller(caller, obj, &i.to_string(), val);
        }
    }
    let _ = define_arguments_data_property(caller, obj, "length", value::encode_f64(len as f64));
    define_arguments_iterator_property(caller, obj);

    if !value::is_undefined(func_ref) {
        let _ = define_arguments_data_property(caller, obj, "callee", func_ref);
    }

    obj
}
