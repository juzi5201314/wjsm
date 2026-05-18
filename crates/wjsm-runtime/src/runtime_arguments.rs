use super::*;

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
    let len = arr_ptr.and_then(|ptr| read_array_length(caller, ptr)).unwrap_or(0);
    let capacity = (len + 1).max(4);
    let obj = alloc_host_object_from_caller(caller, capacity);

    // 覆写 heap type 为 HEAP_TYPE_ARGUMENTS 用于 [object Arguments] 检测
    if let Some(ptr) = resolve_handle(caller, obj) {
        if let Some(Extern::Memory(mem)) = caller.get_export("memory") {
            let data = mem.data_mut(&mut *caller);
            if ptr + 4 < data.len() {
                data[ptr + 4] = wjsm_ir::HEAP_TYPE_ARGUMENTS;
            }
        }
    }

    // 复制参数值作为索引属性
    if let Some(ptr) = arr_ptr {
        for i in 0..len as usize {
            let val = read_array_elem(caller, ptr, i as u32).unwrap_or(value::encode_undefined());
            let _ = define_host_data_property_from_caller(caller, obj, &i.to_string(), val);
        }
    }

    // Set length = 实际参数个数（writable, enumerable=false, configurable=true）
    let _ = define_host_data_property_from_caller(caller, obj, "length", value::encode_f64(len as f64));

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
    let len = arr_ptr.and_then(|ptr| read_array_length(caller, ptr)).unwrap_or(0);
    let capacity = (len + 2).max(4);
    let obj = alloc_host_object_from_caller(caller, capacity);

    // 覆写 heap type 为 HEAP_TYPE_ARGUMENTS 用于 [object Arguments] 检测
    if let Some(ptr) = resolve_handle(caller, obj) {
        if let Some(Extern::Memory(mem)) = caller.get_export("memory") {
            let data = mem.data_mut(&mut *caller);
            if ptr + 4 < data.len() {
                data[ptr + 4] = wjsm_ir::HEAP_TYPE_ARGUMENTS;
            }
        }
    }

    // 复制参数值作为索引属性
    if let Some(ptr) = arr_ptr {
        for i in 0..len as usize {
            let val = read_array_elem(caller, ptr, i as u32).unwrap_or(value::encode_undefined());
            let _ = define_host_data_property_from_caller(caller, obj, &i.to_string(), val);
        }
    }

    // Set length = 实际参数个数
    let _ = define_host_data_property_from_caller(caller, obj, "length", value::encode_f64(len as f64));

    // Set callee = func_ref（仅非严格模式）
    if !value::is_undefined(func_ref) {
        let _ = define_host_data_property_from_caller(caller, obj, "callee", func_ref);
    }

    obj
}
