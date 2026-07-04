use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};
use wjsm_ir::wk_symbol;

use crate::host_imports::get_method::get_by_name_id_sync;
use crate::property_key::encode_symbol_name_id;
use crate::*;
/// Maximum array length per ECMAScript (2^32 - 1).
const MAX_ARRAY_LENGTH: u32 = u32::MAX;
const ARRAY_LENGTH_RANGE_ERROR: &str = "Invalid array length";

fn array_length_would_overflow(len: u32, add: u32) -> bool {
    len.checked_add(add).is_none_or(|n| n > MAX_ARRAY_LENGTH)
}

/// ToUint32（ECMAScript §6.1.6），用于 ArraySetLength 步骤 3。
fn array_length_to_uint32(n: f64) -> u32 {
    if !n.is_finite() || n == 0.0 {
        return 0;
    }
    let v = n.trunc().rem_euclid(4294967296.0);
    v as u32
}

/// ECMAScript §23.1.3.2 ArraySetLength：设置 `array.length`（截断 / 扩展空洞）。
pub(crate) fn array_set_length_impl(
    caller: &mut Caller<'_, RuntimeState>,
    arr: i64,
    len_val: i64,
) -> i64 {
    if !value::is_array(arr) {
        return arr;
    }
    let Some(ptr) = resolve_array_ptr(caller, arr) else {
        return arr;
    };
    let old_len = read_array_length(caller, ptr).unwrap_or(0);

    if same_value_zero(caller, len_val, value::encode_f64(old_len as f64)) {
        return arr;
    }

    let num = to_number(caller, len_val);
    let new_len = array_length_to_uint32(value::decode_f64(num));

    if !same_value_zero(caller, num, value::encode_f64(new_len as f64)) {
        set_runtime_error(
            caller.data(),
            format!("RangeError: {ARRAY_LENGTH_RANGE_ERROR}"),
        );
        return arr;
    }

    if new_len < old_len {
        for i in new_len..old_len {
            write_array_hole(caller, ptr, i);
        }
    } else if new_len > old_len {
        let cap = read_array_capacity(caller, ptr).unwrap_or(0);
        let mut ptr = ptr;
        if new_len > cap {
            let Some(needed) = array_grow_capacity_u32(cap, new_len) else {
                set_runtime_error(
                    caller.data(),
                    format!("RangeError: {ARRAY_LENGTH_RANGE_ERROR}"),
                );
                return arr;
            };
            ptr = grow_array(caller, ptr, arr, needed).unwrap_or(ptr);
        }
        for i in old_len..new_len {
            write_array_hole(caller, ptr, i);
        }
    }

    write_array_length(caller, ptr, new_len);
    arr
}

/// 将容量按倍增策略翻倍（至少为 1），溢出时返回 None。
fn doubled_capacity_u32(cap: u32) -> Option<u32> {
    cap.max(1).checked_mul(2)
}

/// 数组扩容目标容量：翻倍后与 needed 取较大值，且不超过 ECMAScript 数组长度上限。
fn array_grow_capacity_u32(cap: u32, needed: u32) -> Option<u32> {
    let doubled = doubled_capacity_u32(cap)?;
    let grown = needed.max(doubled);
    if grown > MAX_ARRAY_LENGTH {
        None
    } else {
        Some(grown)
    }
}

/// ECMAScript §20.1.2.2 / §20.1.2.21：将 proto 值编码为对象头中的 handle。
fn object_proto_handle_from_value(caller: &mut Caller<'_, RuntimeState>, proto: i64) -> u32 {
    if value::is_null(proto) {
        0xFFFF_FFFF
    } else if value::is_object(proto) {
        value::decode_object_handle(proto)
    } else if value::is_array(proto) {
        value::decode_array_handle(proto)
    } else if value::is_proxy(proto) {
        // 高位标记：proto 链遍历时据此识别 proxy handle 并走 [[Get]] trap
        value::decode_proxy_handle(proto) | 0x8000_0000
    } else if value::is_function(proto) {
        let func_idx = value::decode_function_idx(proto);
        let base = caller
            .get_export("__function_props_base")
            .and_then(|e| e.into_global())
            .and_then(|g| g.get(caller).i32())
            .unwrap_or(0) as u32;
        base.saturating_add(func_idx)
    } else if value::is_closure(proto) {
        let closure_idx = value::decode_closure_idx(proto) as usize;
        let func_idx = caller
            .data()
            .closures
            .lock()
            .ok()
            .and_then(|g| g.get(closure_idx).map(|e| e.func_idx))
            .unwrap_or(0);
        let base = caller
            .get_export("__function_props_base")
            .and_then(|e| e.into_global())
            .and_then(|g| g.get(caller).i32())
            .unwrap_or(0) as u32;
        base.saturating_add(func_idx)
    } else {
        0xFFFF_FFFF
    }
}

fn object_read_current_proto_handle(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
) -> Option<u32> {
    let ptr = resolve_handle(caller, obj)?;
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return None;
    };
    let data = mem.data(caller);
    if ptr + 4 > data.len() {
        return None;
    }
    Some(u32::from_le_bytes([
        data[ptr],
        data[ptr + 1],
        data[ptr + 2],
        data[ptr + 3],
    ]))
}

fn object_write_proto_handle(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    proto_handle: u32,
) -> bool {
    let handle = handle_index_of(caller, obj) as u32;
    let Some(env) = WasmEnv::from_caller(caller) else {
        return false;
    };
    crate::runtime_gc::heap_access::write_proto(caller, &env, handle, proto_handle).is_some()
}

fn object_define_property_or_throw(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    prop: i64,
    desc_handle: i64,
) -> bool {
    let desc = match parse_descriptor(caller, desc_handle) {
        Ok(d) => d,
        Err(msg) => {
            set_runtime_error(caller.data(), msg);
            return false;
        }
    };
    let Some(name_id) = crate::property_key::property_key_value_to_name_id(caller, prop, true)
    else {
        set_runtime_error(caller.data(), "TypeError: Invalid property key".to_string());
        return false;
    };
    match define_property_on_normal_object_for_create(caller, target, name_id, &desc) {
        Ok(_) => true,
        Err(msg) => {
            set_runtime_error(caller.data(), msg);
            false
        }
    }
}

/// 与 `define_property_on_normal_object` 等价的同步 DefineProperty（仅供 Object.create properties）。
fn define_property_on_normal_object_for_create(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    name_id: u32,
    desc: &PropertyDescriptor,
) -> Result<bool, String> {
    if value::is_array(target) {
        if desc.get.is_some() || desc.set.is_some() {
            return Err(
                "TypeError: Accessor properties are not supported on array symbol slots"
                    .to_string(),
            );
        }
        return crate::array_named_props::define_data_property_on_array_named(
            caller, target, name_id, desc,
        );
    }

    let obj_ptr = match resolve_handle(caller, target) {
        Some(p) => p,
        None => return Err("TypeError: Invalid target object".to_string()),
    };
    let found = find_property_slot_by_name_id(caller, obj_ptr, name_id);
    if let Some((slot_offset, old_flags, old_val)) = found {
        let old_configurable = (old_flags & constants::FLAG_CONFIGURABLE) != 0;
        let old_enumerable = (old_flags & constants::FLAG_ENUMERABLE) != 0;
        let old_writable = (old_flags & constants::FLAG_WRITABLE) != 0;
        let old_accessor = (old_flags & constants::FLAG_IS_ACCESSOR) != 0;
        let (old_getter, old_setter) = if old_accessor {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return Err("TypeError: Memory not found".to_string());
            };
            let data = memory.data(&*caller);
            let g =
                i64::from_le_bytes(data[slot_offset + 16..slot_offset + 24].try_into().unwrap());
            let s =
                i64::from_le_bytes(data[slot_offset + 24..slot_offset + 32].try_into().unwrap());
            (g, s)
        } else {
            (value::encode_undefined(), value::encode_undefined())
        };
        if !old_configurable {
            if desc.configurable == Some(true) {
                return Err("TypeError: Cannot redefine non-configurable property".to_string());
            }
            if let Some(new_enum) = desc.enumerable
                && new_enum != old_enumerable
            {
                return Err(
                    "TypeError: Cannot redefine enumerable attribute of non-configurable property"
                        .to_string(),
                );
            }
            let is_new_accessor = desc.get.is_some() || desc.set.is_some();
            if is_new_accessor != old_accessor {
                return Err("TypeError: Cannot change property type from data to accessor or vice versa on non-configurable property".to_string());
            }
            if !old_accessor {
                if !old_writable {
                    if desc.writable == Some(true) {
                        return Err(
                            "TypeError: Cannot make non-writable property writable".to_string()
                        );
                    }
                    if let Some(new_val) = desc.value {
                        let same = strict_eq(caller, old_val, new_val);
                        if value::is_falsy(same) {
                            return Err("TypeError: Cannot change value of non-configurable non-writable property".to_string());
                        }
                    }
                }
            } else {
                if let Some(new_getter) = desc.get
                    && new_getter != old_getter
                {
                    return Err(
                        "TypeError: Cannot change getter of non-configurable property".to_string(),
                    );
                }
                if let Some(new_setter) = desc.set
                    && new_setter != old_setter
                {
                    return Err(
                        "TypeError: Cannot change setter of non-configurable property".to_string(),
                    );
                }
            }
        }
        let is_accessor = desc.get.is_some()
            || desc.set.is_some()
            || (desc.value.is_none() && desc.writable.is_none() && old_accessor);
        let mut flags: i32 = 0;
        if is_accessor {
            flags |= constants::FLAG_IS_ACCESSOR;
        }
        let writable = desc
            .writable
            .unwrap_or(if !is_accessor { old_writable } else { false });
        if writable {
            flags |= constants::FLAG_WRITABLE;
        }
        let enumerable = desc.enumerable.unwrap_or(old_enumerable);
        if enumerable {
            flags |= constants::FLAG_ENUMERABLE;
        }
        let configurable = desc.configurable.unwrap_or(old_configurable);
        if configurable {
            flags |= constants::FLAG_CONFIGURABLE;
        }
        let val = desc.value.unwrap_or(old_val);
        let getter = desc.get.unwrap_or(old_getter);
        let setter = desc.set.unwrap_or(old_setter);
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return Ok(false);
        };
        {
            let data = memory.data_mut(&mut *caller);
            data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
        }
        let handle = handle_index_of(caller, target) as u32;
        let slot_idx = (slot_offset - (obj_ptr + 16)) / 32;
        let Some(env) = WasmEnv::from_caller(caller) else {
            return Ok(false);
        };
        let _ = crate::runtime_gc::heap_access::write_property_slot(
            caller,
            &env,
            handle,
            slot_idx,
            crate::runtime_gc::heap_access::SlotPart::Value,
            val,
        );
        let _ = crate::runtime_gc::heap_access::write_property_slot(
            caller,
            &env,
            handle,
            slot_idx,
            crate::runtime_gc::heap_access::SlotPart::Getter,
            getter,
        );
        let _ = crate::runtime_gc::heap_access::write_property_slot(
            caller,
            &env,
            handle,
            slot_idx,
            crate::runtime_gc::heap_access::SlotPart::Setter,
            setter,
        );
        Ok(true)
    } else {
        if !is_extensible_impl(caller, target) {
            return Err("TypeError: Cannot add property to non-extensible object".to_string());
        }
        let is_accessor = desc.get.is_some() || desc.set.is_some();
        let mut flags: i32 = 0;
        if is_accessor {
            flags |= constants::FLAG_IS_ACCESSOR;
        }
        if desc.writable.unwrap_or(false) && !is_accessor {
            flags |= constants::FLAG_WRITABLE;
        }
        if desc.enumerable.unwrap_or(false) {
            flags |= constants::FLAG_ENUMERABLE;
        }
        if desc.configurable.unwrap_or(false) {
            flags |= constants::FLAG_CONFIGURABLE;
        }
        let val = desc.value.unwrap_or(value::encode_undefined());
        let getter = desc.get.unwrap_or(value::encode_undefined());
        let setter = desc.set.unwrap_or(value::encode_undefined());
        write_new_property_to_memory(caller, target, name_id, flags, val, getter, setter);
        Ok(true)
    }
}

pub(crate) fn object_create_apply_properties(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    properties: i64,
) -> bool {
    if value::is_undefined(properties) {
        return true;
    }
    if !value::is_js_object(properties) {
        set_runtime_error(
            caller.data(),
            "TypeError: Object.create properties must be an object".to_string(),
        );
        return false;
    }
    let Some(props_ptr) = resolve_handle(caller, properties) else {
        return false;
    };
    let string_keys = collect_own_property_names(caller, props_ptr, false);
    for name in string_keys {
        let key_val = store_runtime_string(caller, name.clone());
        let desc_obj = read_object_property_by_string_key_simple(caller, properties, key_val);
        if !object_define_property_or_throw(caller, obj, key_val, desc_obj) {
            return false;
        }
    }
    let symbols = collect_own_property_key_values(caller, props_ptr, true);
    for sym in symbols {
        let desc_obj = read_object_property_by_string_key_simple(caller, properties, sym);
        if !object_define_property_or_throw(caller, obj, sym, desc_obj) {
            return false;
        }
    }
    true
}

fn read_object_property_by_string_key_simple(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    key_val: i64,
) -> i64 {
    let Ok(name) = render_value(caller, key_val) else {
        return value::encode_undefined();
    };
    let Some(ptr) = resolve_handle(caller, obj) else {
        return value::encode_undefined();
    };
    read_object_property_by_name(caller, ptr, &name).unwrap_or_else(value::encode_undefined)
}

/// Array.prototype.join 对单个元素：null/undefined/空洞渲染为空字符串。
pub(crate) fn array_join_element_string(
    caller: &mut Caller<'_, RuntimeState>,
    elem: i64,
) -> String {
    if value::is_null(elem) || value::is_undefined(elem) {
        return String::new();
    }
    render_value(caller, elem).unwrap_or_default()
}

/// 将 fromIndex 规范为 [0, len] 内的起始下标（与 indexOf/includes 共用）。
fn array_relative_start(len: u32, from_index: i64) -> u32 {
    if value::is_undefined(from_index) {
        return 0;
    }
    if !value::is_f64(from_index) {
        return 0;
    }
    let f = value::decode_f64(from_index);
    if f.is_nan() {
        return 0;
    }
    if f == f64::INFINITY {
        return len;
    }
    if f == f64::NEG_INFINITY {
        return 0;
    }
    let len_i = len as i64;
    if f < 0.0 {
        let k = f as i64;
        return (len_i + k).max(0).min(len_i) as u32;
    }
    let k = f as i64;
    k.max(0).min(len_i) as u32
}

pub(crate) fn array_index_of_from(
    caller: &mut Caller<'_, RuntimeState>,
    ptr: usize,
    len: u32,
    search: i64,
    from_index: i64,
) -> i64 {
    let start = array_relative_start(len, from_index);
    for i in start..len {
        if let Some(elem) = read_array_elem(caller, ptr, i)
            && elem == search
        {
            return value::encode_f64(i as f64);
        }
    }
    value::encode_f64(-1.0)
}

pub(crate) fn array_includes_from(
    caller: &mut Caller<'_, RuntimeState>,
    ptr: usize,
    len: u32,
    search: i64,
    from_index: i64,
) -> i64 {
    let start = array_relative_start(len, from_index);
    for i in start..len {
        if let Some(elem) = read_array_elem(caller, ptr, i)
            && same_value_zero(&caller, elem, search)
        {
            return value::encode_bool(true);
        }
    }
    value::encode_bool(false)
}

pub(crate) fn array_last_index_of_from(
    caller: &mut Caller<'_, RuntimeState>,
    ptr: usize,
    len: u32,
    search: i64,
    from_index: i64,
) -> i64 {
    if len == 0 {
        return value::encode_f64(-1.0);
    }
    let len_i = len as i64;
    // fromIndex 缺省为 len-1；ToIntegerOrInfinity 语义（ES2024 §23.1.3.20 步骤 3-5）。
    let f = value::decode_f64(from_index);
    let start = if f.is_nan() {
        0
    } else if f == f64::NEG_INFINITY {
        return value::encode_f64(-1.0);
    } else if f == f64::INFINITY || f >= (len_i - 1) as f64 {
        len_i - 1
    } else if f >= 0.0 {
        f.trunc() as i64
    } else {
        len_i + f.trunc() as i64
    };
    if start < 0 {
        return value::encode_f64(-1.0);
    }
    let mut i = start;
    while i >= 0 {
        if array_elem_present(caller, ptr, i as u32)
            && let Some(elem) = read_array_elem(caller, ptr, i as u32)
            && elem == search
        {
            return value::encode_f64(i as f64);
        }
        i -= 1;
    }
    value::encode_f64(-1.0)
}

/// ECMAScript `Get(O, P)`（同步宿主路径；Proxy / 访问器经 reflect_get）。
fn concat_get_sync(caller: &mut Caller<'_, RuntimeState>, obj: i64, prop: i64) -> i64 {
    let rt = tokio::runtime::Handle::current();
    tokio::task::block_in_place(|| {
        rt.block_on(
            crate::runtime_host_helpers::reflect_get_impl_with_receiver_async(
                caller, obj, prop, obj,
            ),
        )
    })
}

/// ECMAScript §23.1.3.1.1 IsConcatSpreadable ( O )
fn is_concat_spreadable(caller: &mut Caller<'_, RuntimeState>, o: i64) -> Result<bool, i64> {
    if !value::is_js_object(o) {
        return Ok(false);
    }
    let name_id = encode_symbol_name_id(wk_symbol::IS_CONCAT_SPREADABLE);
    let spreadable = get_by_name_id_sync(caller, o, name_id);
    if value::is_exception(spreadable) {
        return Err(spreadable);
    }
    if !value::is_undefined(spreadable) {
        return Ok(nanbox_to_bool(spreadable));
    }
    Ok(value::is_array(o))
}

/// ToLength（§7.1.25）
fn array_concat_to_length(caller: &mut Caller<'_, RuntimeState>, len_val: i64) -> Result<u32, i64> {
    let num = to_number(caller, len_val);
    let f = value::decode_f64(num);
    if !f.is_finite() {
        return Ok(0);
    }
    let int = f.trunc();
    if int < 0.0 {
        return Ok(0);
    }
    const MAX_SAFE: f64 = 9007199254740991.0;
    Ok(array_length_to_uint32(int.min(MAX_SAFE)))
}

fn concat_element_contribution(
    caller: &mut Caller<'_, RuntimeState>,
    e: i64,
) -> Result<usize, i64> {
    if !is_concat_spreadable(caller, e)? {
        return Ok(1);
    }
    if value::is_array(e) {
        if let Some(ptr) = resolve_array_ptr(caller, e) {
            return Ok(read_array_length(caller, ptr).unwrap_or(0) as usize);
        }
        return Ok(0);
    }
    let len_prop = store_runtime_string(caller, "length".to_string());
    let len_val = concat_get_sync(caller, e, len_prop);
    if value::is_exception(len_val) {
        return Err(len_val);
    }
    Ok(array_concat_to_length(caller, len_val)? as usize)
}

fn concat_append_element(
    caller: &mut Caller<'_, RuntimeState>,
    new_ptr: usize,
    write_idx: &mut u32,
    e: i64,
) -> Result<(), i64> {
    if !is_concat_spreadable(caller, e)? {
        write_array_elem(caller, new_ptr, *write_idx, e);
        *write_idx += 1;
        return Ok(());
    }
    if value::is_array(e) {
        if let Some(arg_ptr) = resolve_array_ptr(caller, e) {
            let arg_len = read_array_length(caller, arg_ptr).unwrap_or(0);
            for j in 0..arg_len {
                if let Some(elem) = read_array_elem(caller, arg_ptr, j) {
                    write_array_elem(caller, new_ptr, *write_idx, elem);
                    *write_idx += 1;
                }
            }
        }
        return Ok(());
    }
    let len_prop = store_runtime_string(caller, "length".to_string());
    let len_val = concat_get_sync(caller, e, len_prop);
    if value::is_exception(len_val) {
        return Err(len_val);
    }
    let len_u32 = array_concat_to_length(caller, len_val)?;
    for j in 0..len_u32 {
        let elem = concat_get_sync(caller, e, value::encode_f64(j as f64));
        if value::is_exception(elem) {
            return Err(elem);
        }
        write_array_elem(caller, new_ptr, *write_idx, elem);
        *write_idx += 1;
    }
    Ok(())
}

pub(crate) fn array_concat_two(
    caller: &mut Caller<'_, RuntimeState>,
    left: i64,
    right: i64,
) -> i64 {
    let Some(left_ptr) = resolve_array_ptr(caller, left) else {
        return value::encode_undefined();
    };
    let left_len = read_array_length(caller, left_ptr).unwrap_or(0);
    let add_right = match concat_element_contribution(caller, right) {
        Ok(n) => n,
        Err(exc) => return exc,
    };
    let Some(total_len) = (left_len as usize).checked_add(add_right) else {
        return make_range_error_exception(caller, ARRAY_LENGTH_RANGE_ERROR);
    };
    let Ok(total_len_u32) = u32::try_from(total_len) else {
        return make_range_error_exception(caller, ARRAY_LENGTH_RANGE_ERROR);
    };
    let new_arr = array_species_create(caller, left, total_len_u32);
    let Some(new_ptr) = resolve_array_ptr(caller, new_arr) else {
        return value::encode_undefined();
    };
    let mut write_idx = 0u32;
    for i in 0..left_len {
        if let Some(elem) = read_array_elem(caller, left_ptr, i) {
            write_array_elem(caller, new_ptr, write_idx, elem);
            write_idx += 1;
        }
    }
    if let Err(exc) = concat_append_element(caller, new_ptr, &mut write_idx, right) {
        return exc;
    }
    write_array_length(caller, new_ptr, write_idx);
    new_arr
}

pub(crate) fn array_concat_args(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    if resolve_array_ptr(caller, this_val).is_none() {
        return value::encode_undefined();
    };
    let mut total_len = 0usize;
    let mut items: Vec<i64> = Vec::with_capacity(1 + args_count.max(0) as usize);
    items.push(this_val);
    for i in 0..args_count as u32 {
        items.push(read_shadow_arg(caller, args_base, i));
    }
    for &e in &items {
        let add = match concat_element_contribution(caller, e) {
            Ok(n) => n,
            Err(exc) => return exc,
        };
        let Some(next_len) = total_len.checked_add(add) else {
            return make_range_error_exception(caller, ARRAY_LENGTH_RANGE_ERROR);
        };
        total_len = next_len;
    }
    let Ok(total_len_u32) = u32::try_from(total_len) else {
        return make_range_error_exception(caller, ARRAY_LENGTH_RANGE_ERROR);
    };
    let new_arr = array_species_create(caller, this_val, total_len_u32);
    let Some(new_ptr) = resolve_array_ptr(caller, new_arr) else {
        return value::encode_undefined();
    };
    let mut write_idx = 0u32;
    for &e in &items {
        if let Err(exc) = concat_append_element(caller, new_ptr, &mut write_idx, e) {
            return exc;
        }
    }
    write_array_length(caller, new_ptr, write_idx);
    new_arr
}

pub(crate) fn array_slice_range(
    caller: &mut Caller<'_, RuntimeState>,
    arr: i64,
    start_arg: i64,
    end_arg: i64,
) -> i64 {
    let Some(ptr) = resolve_array_ptr(caller, arr) else {
        return value::encode_undefined();
    };
    let len = read_array_length(caller, ptr).unwrap_or(0) as i32;
    let start = if value::is_undefined(start_arg) {
        0
    } else if value::is_f64(start_arg) {
        let s_f64 = value::decode_f64(start_arg);
        if s_f64.is_nan() {
            0
        } else if s_f64 < 0.0 {
            (len + s_f64 as i32).max(0)
        } else {
            (s_f64 as i32).min(len)
        }
    } else {
        0
    };
    let end = if value::is_undefined(end_arg) {
        len
    } else if value::is_f64(end_arg) {
        let e_f64 = value::decode_f64(end_arg);
        if e_f64.is_nan() {
            len
        } else if e_f64 < 0.0 {
            (len + e_f64 as i32).max(0)
        } else {
            (e_f64 as i32).min(len)
        }
    } else {
        len
    };
    let count = (end - start).max(0) as u32;
    let new_arr = array_species_create(caller, arr, count);
    let Some(new_ptr) = resolve_array_ptr(caller, new_arr) else {
        return value::encode_undefined();
    };
    for i in 0..count {
        let elem =
            read_array_elem(caller, ptr, start as u32 + i).unwrap_or(value::encode_undefined());
        write_array_elem(caller, new_ptr, i, elem);
    }
    write_array_length(caller, new_ptr, count);
    new_arr
}

/// Array.prototype.fill 的宿主实现：在 [start, end) 范围内写入 val。
pub(crate) fn array_fill_range(
    caller: &mut Caller<'_, RuntimeState>,
    arr: i64,
    val: i64,
    start_arg: i64,
    end_arg: i64,
) -> i64 {
    let Some(ptr) = resolve_array_ptr(caller, arr) else {
        return arr;
    };
    let len = read_array_length(caller, ptr).unwrap_or(0) as i32;
    let start = if value::is_undefined(start_arg) {
        0
    } else if value::is_f64(start_arg) {
        let s_f64 = value::decode_f64(start_arg);
        if s_f64.is_nan() {
            0
        } else if s_f64 < 0.0 {
            (len + s_f64 as i32).max(0)
        } else {
            (s_f64 as i32).min(len)
        }
    } else {
        0
    };
    let end = if value::is_undefined(end_arg) {
        len
    } else if value::is_f64(end_arg) {
        let e_f64 = value::decode_f64(end_arg);
        if e_f64.is_nan() {
            len
        } else if e_f64 < 0.0 {
            (len + e_f64 as i32).max(0)
        } else {
            (e_f64 as i32).min(len)
        }
    } else {
        len
    };
    for i in start..end {
        write_array_elem(caller, ptr, i as u32, val);
    }
    arr
}

/// Array.prototype.flat 的宿主实现：按 depth 展平数组，返回新数组。
pub(crate) fn array_flat_with_depth(
    caller: &mut Caller<'_, RuntimeState>,
    arr: i64,
    depth_arg: i64,
) -> i64 {
    let depth = if value::is_undefined(depth_arg) {
        1u32
    } else if value::is_f64(depth_arg) {
        let d = value::decode_f64(depth_arg);
        if d.is_nan() {
            0
        } else {
            let i = d.trunc() as i64;
            if i <= 0 { 0 } else { i as u32 }
        }
    } else {
        1
    };
    fn flatten(
        caller: &mut Caller<'_, RuntimeState>,
        arr: i64,
        depth: u32,
        elements: &mut Vec<i64>,
    ) {
        let Some(ptr) = resolve_array_ptr(caller, arr) else {
            elements.push(arr);
            return;
        };
        let len = read_array_length(caller, ptr).unwrap_or(0);
        for i in 0..len {
            if let Some(elem) = read_array_elem(caller, ptr, i) {
                if depth > 0 && value::is_array(elem) {
                    flatten(caller, elem, depth - 1, elements);
                } else {
                    elements.push(elem);
                }
            }
        }
    }
    let mut elements = Vec::new();
    flatten(caller, arr, depth, &mut elements);
    let new_arr = array_species_create(caller, arr, elements.len() as u32);
    let Some(new_ptr) = resolve_array_ptr(caller, new_arr) else {
        return value::encode_undefined();
    };
    for (i, elem) in elements.iter().enumerate() {
        write_array_elem(caller, new_ptr, i as u32, *elem);
    }
    write_array_length(caller, new_ptr, elements.len() as u32);
    new_arr
}

pub(crate) fn push_array_value(
    caller: &mut Caller<'_, RuntimeState>,
    arr: i64,
    val: i64,
) -> Result<(), i64> {
    let mut ptr = resolve_array_ptr(caller, arr).ok_or_else(value::encode_undefined)?;
    let len = read_array_length(caller, ptr).unwrap_or(0);
    if array_length_would_overflow(len, 1) {
        return Err(make_range_error_exception(caller, ARRAY_LENGTH_RANGE_ERROR));
    }
    let cap = read_array_capacity(caller, ptr).unwrap_or(0);
    if len >= cap {
        let Some(needed) = array_grow_capacity_u32(cap, len + 1) else {
            return Err(make_range_error_exception(caller, ARRAY_LENGTH_RANGE_ERROR));
        };
        ptr = grow_array(caller, ptr, arr, needed).ok_or_else(value::encode_undefined)?;
    }
    write_array_elem(caller, ptr, len, val);
    write_array_length(caller, ptr, len + 1);
    Ok(())
}

/// 数组字面量 elision：length+1 并在新下标写入 hole。
pub(crate) fn push_array_hole(caller: &mut Caller<'_, RuntimeState>, arr: i64) -> Result<(), i64> {
    let mut ptr = resolve_array_ptr(caller, arr).ok_or_else(value::encode_undefined)?;
    let len = read_array_length(caller, ptr).unwrap_or(0);
    if array_length_would_overflow(len, 1) {
        return Err(make_range_error_exception(caller, ARRAY_LENGTH_RANGE_ERROR));
    }
    let cap = read_array_capacity(caller, ptr).unwrap_or(0);
    if len >= cap {
        let Some(needed) = array_grow_capacity_u32(cap, len + 1) else {
            return Err(make_range_error_exception(caller, ARRAY_LENGTH_RANGE_ERROR));
        };
        ptr = grow_array(caller, ptr, arr, needed).ok_or_else(value::encode_undefined)?;
    }
    write_array_hole(caller, ptr, len);
    write_array_length(caller, ptr, len + 1);
    Ok(())
}

async fn push_iterator_values_async(
    caller: &mut Caller<'_, RuntimeState>,
    arr: i64,
    iterator: i64,
) -> bool {
    let Some(iter_ptr) = resolve_handle(caller, iterator) else {
        return false;
    };
    let Some(next) = read_object_property_by_name(caller, iter_ptr, "next") else {
        return false;
    };
    if !value::is_callable(next) {
        return false;
    }
    loop {
        let result =
            call_iterator_method_async(caller, next, iterator, value::encode_undefined()).await;

        // A4: 若 next() 同步抛出（返回 TAG_EXCEPTION），用真实错误消息替换误导的 "not iterable"。
        // 注：表达式位 spread 无 IsException 分叉，无法做到可捕获；仅改进延迟错误消息的准确性。
        if value::is_exception(result) {
            let reason = exception_reason(caller, result);
            let msg = render_value(caller, reason).unwrap_or_else(|_| "unknown error".to_string());
            set_runtime_error(
                caller.data(),
                format!("TypeError: iterator.next() threw: {}", msg),
            );
            return false;
        }

        let Some(result_ptr) = resolve_handle(caller, result) else {
            return false;
        };
        let done = read_object_property_by_name(caller, result_ptr, "done")
            .map(nanbox_to_bool)
            .unwrap_or(true);
        if done {
            return true;
        }
        let val = read_object_property_by_name(caller, result_ptr, "value")
            .unwrap_or_else(value::encode_undefined);
        if push_array_value(caller, arr, val).is_err() {
            set_runtime_error(caller.data(), ARRAY_LENGTH_RANGE_ERROR.to_string());
            return false;
        }
    }
}

pub(crate) async fn array_push_spread_impl_async(
    caller: &mut Caller<'_, RuntimeState>,
    arr: i64,
    iterable: i64,
) -> i64 {
    if value::is_array(iterable)
        && let Some(ptr) = resolve_array_ptr(caller, iterable)
    {
        let len = read_array_length(caller, ptr).unwrap_or(0);
        for i in 0..len {
            let val = read_array_elem(caller, ptr, i).unwrap_or_else(value::encode_undefined);
            if push_array_value(caller, arr, val).is_err() {
                set_runtime_error(caller.data(), ARRAY_LENGTH_RANGE_ERROR.to_string());
                return value::encode_undefined();
            }
        }
        return value::encode_undefined();
    }

    if value::is_string(iterable) {
        let string = get_string_value(caller, iterable);
        let mut unit_pos = 0usize;
        while unit_pos < string.utf16_len() {
            let val = super::string_iter_current_value(caller, &string, unit_pos);
            super::string_iter_advance_unit_pos(&string, &mut unit_pos);
            if push_array_value(caller, arr, val).is_err() {
                set_runtime_error(caller.data(), ARRAY_LENGTH_RANGE_ERROR.to_string());
                return value::encode_undefined();
            }
        }
        return value::encode_undefined();
    }

    if let Some(ptr) = resolve_handle(caller, iterable)
        && let Some(method) = read_iterator_method(caller, ptr)
    {
        let iterator = call_iterable_method_async(caller, method, iterable).await;
        if push_iterator_values_async(caller, arr, iterator).await {
            return value::encode_undefined();
        }
    }

    set_runtime_error(
        caller.data(),
        "TypeError: value is not iterable".to_string(),
    );
    value::encode_undefined()
}
/// ECMAScript §23.1.2.1 `Array.from(items, mapFn?)`：支持可迭代对象（@@iterator）、
/// 类数组（length + 索引）、字符串、TypedArray；可选 mapFn 对每个元素调用 `mapFn(v, i)`。
pub(crate) async fn array_from_impl_async(
    caller: &mut Caller<'_, RuntimeState>,
    source: i64,
    map_fn: i64,
) -> i64 {
    let has_map_fn = value::is_callable(map_fn);
    let values = match collect_array_from_values_async(caller, source).await {
        Some(v) => v,
        None => {
            set_runtime_error(
                caller.data(),
                "TypeError: Array.from requires an array-like or iterable object".to_string(),
            );
            return value::encode_undefined();
        }
    };

    let count = values.len() as u32;
    let result = alloc_array(caller, count);
    if resolve_array_ptr(caller, result).is_none() {
        return value::encode_undefined();
    }
    for (i, raw) in values.into_iter().enumerate() {
        let mapped = if has_map_fn {
            let idx_val = value::encode_f64(i as f64);
            let r = call_wasm_callback_async(
                caller,
                map_fn,
                value::encode_undefined(),
                &[raw, idx_val],
            )
            .await
            .unwrap_or_else(|_| value::encode_undefined());
            if value::is_exception(r) {
                return r;
            }
            r
        } else {
            raw
        };
        // 回调可能触发 GC 搬移数组，重新解析指针后再写入。
        let Some(arr_ptr) = resolve_array_ptr(caller, result) else {
            return value::encode_undefined();
        };
        write_array_elem(caller, arr_ptr, i as u32, mapped);
    }
    let Some(arr_ptr) = resolve_array_ptr(caller, result) else {
        return value::encode_undefined();
    };
    write_array_length(caller, arr_ptr, count);
    result
}

/// 将 Array.from 的源收集为值序列。返回 None 表示既非可迭代也非类数组。
async fn collect_array_from_values_async(
    caller: &mut Caller<'_, RuntimeState>,
    source: i64,
) -> Option<Vec<i64>> {
    // 裸 TAG_ITERATOR（如 arr[Symbol.iterator]()）：按内部状态抽干非 reentrant 迭代器。
    if value::is_iterator(source) {
        return Some(drain_raw_iterator_values(caller, source));
    }
    // 数组快速路径
    if value::is_array(source)
        && let Some(ptr) = resolve_array_ptr(caller, source)
    {
        let len = read_array_length(caller, ptr).unwrap_or(0);
        let mut out = Vec::with_capacity(len as usize);
        for i in 0..len {
            out.push(read_array_elem(caller, ptr, i).unwrap_or_else(value::encode_undefined));
        }
        return Some(out);
    }
    // 字符串：按码点
    if value::is_string(source) {
        let string = get_string_value(caller, source);
        let mut out = Vec::new();
        let mut unit_pos = 0usize;
        while unit_pos < string.utf16_len() {
            out.push(super::string_iter_current_value(caller, &string, unit_pos));
            super::string_iter_advance_unit_pos(&string, &mut unit_pos);
        }
        return Some(out);
    }
    // TypedArray
    if let Some(entry) = typedarray_entry_from_value(caller, source) {
        let mut out = Vec::with_capacity(entry.length as usize);
        for i in 0..entry.length {
            out.push(
                typedarray_element_read(caller, source, i).unwrap_or_else(value::encode_undefined),
            );
        }
        return Some(out);
    }
    // 可迭代对象：通过 @@iterator + 迭代协议抽干
    if let Some(ptr) = resolve_handle(caller, source)
        && let Some(method) = read_iterator_method(caller, ptr)
    {
        let iterator = call_iterable_method_async(caller, method, source).await;
        return collect_iterator_values_async(caller, iterator).await;
    }
    // 类数组：读取 length + 索引属性
    if value::is_object(source) || value::is_function(source) {
        if let Some(ptr) = resolve_handle(caller, source) {
            let len_val = read_object_property_by_name(caller, ptr, "length")
                .unwrap_or_else(value::encode_undefined);
            let len = array_concat_to_length(caller, len_val).unwrap_or(0);
            let mut out = Vec::with_capacity(len as usize);
            for i in 0..len {
                let key = i.to_string();
                let cur = resolve_handle(caller, source)
                    .and_then(|p| read_object_property_by_name(caller, p, &key))
                    .unwrap_or_else(value::encode_undefined);
                out.push(cur);
            }
            return Some(out);
        }
    }
    None
}
/// 抽干裸 TAG_ITERATOR 的非 reentrant 内部状态为值序列（供 Array.from 处理
/// `arr[Symbol.iterator]()` 等直接传入的迭代器）。reentrant 的 ObjectIter 不在此处理，
/// 因为它需要异步回调 next()，应由 @@iterator 协议路径处理可迭代对象本身。
fn drain_raw_iterator_values(caller: &mut Caller<'_, RuntimeState>, iterator: i64) -> Vec<i64> {
    let handle_idx = value::decode_handle(iterator) as usize;
    let mut out = Vec::new();
    loop {
        // 判定 done 并推进 index；非支持状态则停止。
        let done = {
            let mut iters = caller
                .data()
                .iterators
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(iter) = iters.get_mut(handle_idx) else {
                break;
            };
            match iter {
                IteratorState::StringIter { string, unit_pos } => *unit_pos >= string.utf16_len(),
                IteratorState::ArrayIter { index, length, .. } => *index >= *length,
                IteratorState::IndexValueIter { index, values } => *index as usize >= values.len(),
                IteratorState::MapKeyIter {
                    index, map_handle, ..
                }
                | IteratorState::MapValueIter {
                    index, map_handle, ..
                }
                | IteratorState::MapEntryIter {
                    index, map_handle, ..
                } => {
                    let table = caller
                        .data()
                        .map_table
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    *map_handle >= table.len() as u32
                        || *index as usize >= table[*map_handle as usize].keys.len()
                }
                IteratorState::SetValueIter {
                    index, set_handle, ..
                }
                | IteratorState::SetEntryIter {
                    index, set_handle, ..
                } => {
                    let table = caller
                        .data()
                        .set_table
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    *set_handle >= table.len() as u32
                        || *index as usize >= table[*set_handle as usize].values.len()
                }
                IteratorState::TypedArrayValueIter { index, length, .. }
                | IteratorState::TypedArrayEntryIter { index, length, .. } => *index >= *length,
                IteratorState::RegExpStringIter { .. } => {
                    drop(iters);
                    regexp_string_iter_ensure_current(caller, handle_idx)
                }
                // reentrant / 未知状态：停止抽干。
                _ => true,
            }
        };
        if done {
            break;
        }
        let val = super::core::iterator_value_impl(caller, iterator);
        out.push(val);
        advance_raw_iterator(caller, handle_idx);
    }
    out
}

/// 推进裸迭代器的内部 index（与 drain_raw_iterator_values 配套）。
fn advance_raw_iterator(caller: &mut Caller<'_, RuntimeState>, handle_idx: usize) {
    let mut iters = caller
        .data()
        .iterators
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(iter) = iters.get_mut(handle_idx) else {
        return;
    };
    match iter {
        IteratorState::StringIter { string, unit_pos } => {
            super::string_iter_advance_unit_pos(string, unit_pos);
        }
        IteratorState::ArrayIter { index, .. }
        | IteratorState::IndexValueIter { index, .. }
        | IteratorState::MapKeyIter { index, .. }
        | IteratorState::MapValueIter { index, .. }
        | IteratorState::MapEntryIter { index, .. }
        | IteratorState::SetValueIter { index, .. }
        | IteratorState::SetEntryIter { index, .. } => {
            *index += 1;
        }
        IteratorState::TypedArrayValueIter { index, .. }
        | IteratorState::TypedArrayEntryIter { index, .. } => {
            *index += 1;
        }
        IteratorState::RegExpStringIter { .. } => {
            drop(iters);
            regexp_string_iter_next(caller, handle_idx);
        }
        _ => {}
    }
}

/// 抽干一个迭代器（next/done/value 协议）为值序列。
async fn collect_iterator_values_async(
    caller: &mut Caller<'_, RuntimeState>,
    iterator: i64,
) -> Option<Vec<i64>> {
    let iter_ptr = resolve_handle(caller, iterator)?;
    let next = read_object_property_by_name(caller, iter_ptr, "next")?;
    if !value::is_callable(next) {
        return None;
    }
    let mut out = Vec::new();
    loop {
        let result =
            call_iterator_method_async(caller, next, iterator, value::encode_undefined()).await;
        if value::is_exception(result) {
            return Some(out);
        }
        let Some(result_ptr) = resolve_handle(caller, result) else {
            return Some(out);
        };
        let done = read_object_property_by_name(caller, result_ptr, "done")
            .map(nanbox_to_bool)
            .unwrap_or(true);
        if done {
            return Some(out);
        }
        let val = read_object_property_by_name(caller, result_ptr, "value")
            .unwrap_or_else(value::encode_undefined);
        out.push(val);
    }
}

/// `Object.fromEntries(iterable)`：从 [key, value] 可迭代序列创建普通对象。
pub(crate) async fn object_from_entries_impl_async(
    caller: &mut Caller<'_, RuntimeState>,
    iterable: i64,
) -> i64 {
    if value::is_null(iterable) || value::is_undefined(iterable) {
        set_runtime_error(
            caller.data(),
            "TypeError: Cannot convert undefined or null to object".to_string(),
        );
        return value::encode_undefined();
    }
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let result = alloc_host_object(caller, &env, 0);

    let mut entries: Vec<(i64, i64)> = Vec::new();
    if value::is_array(iterable)
        && let Some(arr_ptr) = resolve_array_ptr(caller, iterable)
    {
        let len = read_array_length(caller, arr_ptr).unwrap_or(0);
        for i in 0..len {
            let pair = read_array_elem(caller, arr_ptr, i).unwrap_or_else(value::encode_undefined);
            if !value::is_array(pair) {
                set_runtime_error(
                    caller.data(),
                    "TypeError: Iterator value is not an entry object".to_string(),
                );
                return value::encode_undefined();
            }
            let Some(pair_ptr) = resolve_array_ptr(caller, pair) else {
                continue;
            };
            let key_elem =
                read_array_elem(caller, pair_ptr, 0).unwrap_or_else(value::encode_undefined);
            let val_elem =
                read_array_elem(caller, pair_ptr, 1).unwrap_or_else(value::encode_undefined);
            entries.push((key_elem, val_elem));
        }
    } else if let Some(ptr) = resolve_handle(caller, iterable)
        && let Some(method) = read_iterator_method(caller, ptr)
    {
        let iterator = call_iterable_method_async(caller, method, iterable).await;
        let pairs = match collect_iterator_values_async(caller, iterator).await {
            Some(v) => v,
            None => {
                set_runtime_error(
                    caller.data(),
                    "TypeError: value is not iterable".to_string(),
                );
                return value::encode_undefined();
            }
        };
        for pair in pairs {
            if !value::is_array(pair) {
                set_runtime_error(
                    caller.data(),
                    "TypeError: Iterator value is not an entry object".to_string(),
                );
                return value::encode_undefined();
            }
            let Some(pair_ptr) = resolve_array_ptr(caller, pair) else {
                continue;
            };
            let key_elem =
                read_array_elem(caller, pair_ptr, 0).unwrap_or_else(value::encode_undefined);
            let val_elem =
                read_array_elem(caller, pair_ptr, 1).unwrap_or_else(value::encode_undefined);
            entries.push((key_elem, val_elem));
        }
    } else {
        set_runtime_error(
            caller.data(),
            "TypeError: value is not iterable".to_string(),
        );
        return value::encode_undefined();
    }

    for (key_elem, val_elem) in entries {
        if let Some(name_id) =
            crate::property_key::property_key_value_to_name_id(caller, key_elem, true)
        {
            let _ = define_host_data_property_by_name_id(caller, result, name_id, val_elem);
        }
    }
    result
}

/// `Object.getOwnPropertyDescriptors(obj)`：返回所有自有属性描述符对象。
pub(crate) fn object_get_own_property_descriptors_impl(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
) -> i64 {
    if value::is_null(target) || value::is_undefined(target) {
        set_runtime_error(
            caller.data(),
            "TypeError: Cannot convert undefined or null to object".to_string(),
        );
        return value::encode_undefined();
    }
    let boxed = if value::is_js_object(target) {
        target
    } else {
        to_object(caller, target)
    };
    let keys_arr = super::proxy_reflect::reflect_own_keys_impl(caller, boxed);
    let Some(keys_ptr) = resolve_array_ptr(caller, keys_arr) else {
        return value::encode_undefined();
    };
    let len = read_array_length(caller, keys_ptr).unwrap_or(0);
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let result = alloc_host_object(caller, &env, 0);
    for i in 0..len {
        let key = read_array_elem(caller, keys_ptr, i).unwrap_or_else(value::encode_undefined);
        let desc =
            super::proxy_reflect::reflect_get_own_property_descriptor_impl(caller, boxed, key);
        if value::is_undefined(desc) {
            continue;
        }
        if let Some(name_id) = crate::property_key::property_key_value_to_name_id(caller, key, true)
        {
            let _ = define_host_data_property_by_name_id(caller, result, name_id, desc);
        }
    }
    result
}

pub(crate) fn define_array_object(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let arr_proto_push_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let count = args_count as u32;
            if array_length_would_overflow(len, count) {
                return make_range_error_exception(&mut caller, ARRAY_LENGTH_RANGE_ERROR);
            }
            let cap = read_array_capacity(&mut caller, ptr).unwrap_or(0);
            let mut ptr = ptr;
            if len + count > cap {
                let Some(needed) = array_grow_capacity_u32(cap, len + count) else {
                    return make_range_error_exception(&mut caller, ARRAY_LENGTH_RANGE_ERROR);
                };
                if let Some(new_ptr) = grow_array(&mut caller, ptr, this_val, needed) {
                    ptr = new_ptr;
                } else {
                    return value::encode_undefined();
                }
            }
            for i in 0..count {
                let val = read_shadow_arg(&mut caller, args_base, i);
                write_array_elem(&mut caller, ptr, len + i, val);
            }
            write_array_length(&mut caller, ptr, len + count);
            value::encode_f64((len + count) as f64)
        },
    );
    linker.define(&mut store, "env", "arr_proto_push", arr_proto_push_fn)?;

    // ── arr_proto_pop (#50) ───────────────────────────────────────────
    let arr_proto_pop_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         _args_base: i32,
         _args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            if len == 0 {
                return value::encode_undefined();
            }
            let new_len = len - 1;
            let val =
                read_array_elem(&mut caller, ptr, new_len).unwrap_or(value::encode_undefined());
            write_array_length(&mut caller, ptr, new_len);
            val
        },
    );
    linker.define(&mut store, "env", "arr_proto_pop", arr_proto_pop_fn)?;

    // ── arr_proto_includes (#51) ──────────────────────────────────────
    let arr_proto_includes_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let search = read_shadow_arg(&mut caller, args_base, 0);
            let from_index = if args_count > 1 {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_bool(false);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            array_includes_from(&mut caller, ptr, len, search, from_index)
        },
    );
    linker.define(
        &mut store,
        "env",
        "arr_proto_includes",
        arr_proto_includes_fn,
    )?;

    // ── arr_proto_index_of (#52) ──────────────────────────────────────
    let arr_proto_index_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let search = read_shadow_arg(&mut caller, args_base, 0);
            let from_index = if args_count > 1 {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_f64(-1.0);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            array_index_of_from(&mut caller, ptr, len, search, from_index)
        },
    );
    linker.define(
        &mut store,
        "env",
        "arr_proto_index_of",
        arr_proto_index_of_fn,
    )?;

    // ── arr_proto_join (#53) ─────────────────────────────────────────
    let arr_proto_join_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let sep_val = if args_count > 0 {
                read_shadow_arg(&mut caller, args_base, 0)
            } else {
                value::encode_undefined()
            };
            // 默认分隔符为 ","
            let sep_str = if value::is_undefined(sep_val) || value::is_null(sep_val) {
                ",".to_string()
            } else {
                get_string_utf8_lossy(&mut caller, sep_val)
            };
            let mut parts = Vec::new();
            for i in 0..len {
                if let Some(elem) = read_array_elem(&mut caller, ptr, i) {
                    parts.push(array_join_element_string(&mut caller, elem));
                } else {
                    parts.push(String::new());
                }
            }
            store_runtime_string(&caller, parts.join(&sep_str))
        },
    );
    linker.define(&mut store, "env", "arr_proto_join", arr_proto_join_fn)?;

    // ── arr_proto_concat (#54) ────────────────────────────────────────
    let arr_proto_concat_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 { array_concat_args(&mut caller, this_val, args_base, args_count) },
    );
    linker.define(&mut store, "env", "arr_proto_concat", arr_proto_concat_fn)?;

    // ── arr_proto_slice (#55) ─────────────────────────────────────────
    let arr_proto_slice_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            let start = if args_count > 0 {
                let s_f64 = value::decode_f64(read_shadow_arg(&mut caller, args_base, 0));
                if s_f64.is_nan() {
                    0
                } else if s_f64 < 0.0 {
                    (len + s_f64 as i32).max(0)
                } else {
                    (s_f64 as i32).min(len)
                }
            } else {
                0
            };
            let end = if args_count > 1 {
                let e_f64 = value::decode_f64(read_shadow_arg(&mut caller, args_base, 1));
                if e_f64.is_nan() {
                    len
                } else if e_f64 < 0.0 {
                    (len + e_f64 as i32).max(0)
                } else {
                    (e_f64 as i32).min(len)
                }
            } else {
                len
            };
            let count = (end - start).max(0) as u32;
            let new_arr = array_species_create(&mut caller, this_val, count);
            let Some(new_ptr) = resolve_array_ptr(&mut caller, new_arr) else {
                return value::encode_undefined();
            };
            for i in 0..count {
                let elem = read_array_elem(&mut caller, ptr, start as u32 + i)
                    .unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, new_ptr, i, elem);
            }
            write_array_length(&mut caller, new_ptr, count);
            new_arr
        },
    );
    linker.define(&mut store, "env", "arr_proto_slice", arr_proto_slice_fn)?;

    // ── arr_proto_fill (#56) ──────────────────────────────────────────
    let arr_proto_fill_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let val = read_shadow_arg(&mut caller, args_base, 0);
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return this_val;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            let start = if args_count > 1 {
                let s_f64 = value::decode_f64(read_shadow_arg(&mut caller, args_base, 1));
                if s_f64.is_nan() {
                    0
                } else if s_f64 < 0.0 {
                    (len + s_f64 as i32).max(0)
                } else {
                    (s_f64 as i32).min(len)
                }
            } else {
                0
            };
            let end = if args_count > 2 {
                let e_f64 = value::decode_f64(read_shadow_arg(&mut caller, args_base, 2));
                if e_f64.is_nan() {
                    len
                } else if e_f64 < 0.0 {
                    (len + e_f64 as i32).max(0)
                } else {
                    (e_f64 as i32).min(len)
                }
            } else {
                len
            };
            for i in start..end {
                write_array_elem(&mut caller, ptr, i as u32, val);
            }
            this_val
        },
    );
    linker.define(&mut store, "env", "arr_proto_fill", arr_proto_fill_fn)?;

    // ── arr_proto_reverse (#57) ───────────────────────────────────────
    let arr_proto_reverse_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         _args_base: i32,
         _args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return this_val;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len / 2 {
                let a = read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let b = read_array_elem(&mut caller, ptr, len - 1 - i)
                    .unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, ptr, i, b);
                write_array_elem(&mut caller, ptr, len - 1 - i, a);
            }
            this_val
        },
    );
    linker.define(&mut store, "env", "arr_proto_reverse", arr_proto_reverse_fn)?;

    // ── arr_proto_flat (#58) ──────────────────────────────────────────
    let arr_proto_flat_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            // depth: default 1; ToIntegerOrInfinity; depth <= 0 means no flattening
            let depth = if args_count > 0 {
                let d = value::decode_f64(read_shadow_arg(&mut caller, args_base, 0));
                if d.is_nan() {
                    0
                } else {
                    let i = d.trunc() as i64;
                    if i <= 0 { 0 } else { i as u32 }
                }
            } else {
                1
            };
            // 递归展平
            fn flatten(
                caller: &mut Caller<'_, RuntimeState>,
                arr: i64,
                depth: u32,
                elements: &mut Vec<i64>,
            ) {
                let Some(ptr) = resolve_array_ptr(caller, arr) else {
                    elements.push(arr);
                    return;
                };
                let len = read_array_length(caller, ptr).unwrap_or(0);
                for i in 0..len {
                    if let Some(elem) = read_array_elem(caller, ptr, i) {
                        if depth > 0 && value::is_array(elem) {
                            flatten(caller, elem, depth - 1, elements);
                        } else {
                            elements.push(elem);
                        }
                    }
                }
            }
            let mut elements = Vec::new();
            flatten(&mut caller, this_val, depth, &mut elements);
            let new_arr = array_species_create(&mut caller, this_val, elements.len() as u32);
            let Some(new_ptr) = resolve_array_ptr(&mut caller, new_arr) else {
                return value::encode_undefined();
            };
            for (i, elem) in elements.iter().enumerate() {
                write_array_elem(&mut caller, new_ptr, i as u32, *elem);
            }
            write_array_length(&mut caller, new_ptr, elements.len() as u32);
            new_arr
        },
    );
    linker.define(&mut store, "env", "arr_proto_flat", arr_proto_flat_fn)?;

    // ── arr_proto_shift (#59) ─────────────────────────────────────────
    let arr_proto_shift_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         _args_base: i32,
         _args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            if len == 0 {
                return value::encode_undefined();
            }
            let val = read_array_elem(&mut caller, ptr, 0).unwrap_or(value::encode_undefined());
            // 左移元素
            for i in 1..len {
                let elem =
                    read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, ptr, i - 1, elem);
            }
            write_array_length(&mut caller, ptr, len - 1);
            val
        },
    );
    linker.define(&mut store, "env", "arr_proto_shift", arr_proto_shift_fn)?;

    // ── arr_proto_unshift (#60) ───────────────────────────────────────
    let arr_proto_unshift_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let add = args_count as u32;
            if array_length_would_overflow(len, add) {
                return make_range_error_exception(&mut caller, ARRAY_LENGTH_RANGE_ERROR);
            }
            let cap = read_array_capacity(&mut caller, ptr).unwrap_or(0);
            let new_len = len + add;
            let mut ptr = ptr;
            if new_len > cap {
                let Some(needed) = array_grow_capacity_u32(cap, new_len) else {
                    return make_range_error_exception(&mut caller, ARRAY_LENGTH_RANGE_ERROR);
                };
                if let Some(new_ptr) = grow_array(&mut caller, ptr, this_val, needed) {
                    ptr = new_ptr;
                } else {
                    return value::encode_undefined();
                }
            }
            // 右移现有元素
            for i in (0..len).rev() {
                let elem =
                    read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, ptr, i + args_count as u32, elem);
            }
            // 在前面插入新元素
            for i in 0..args_count as u32 {
                let arg = read_shadow_arg(&mut caller, args_base, i);
                write_array_elem(&mut caller, ptr, i, arg);
            }
            write_array_length(&mut caller, ptr, new_len);
            value::encode_f64(new_len as f64)
        },
    );
    linker.define(&mut store, "env", "arr_proto_unshift", arr_proto_unshift_fn)?;

    // ── arr_proto_at (#62) ────────────────────────────────────────────
    let arr_proto_at_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            let idx = if args_count > 0 {
                let i_f64 = value::decode_f64(read_shadow_arg(&mut caller, args_base, 0));
                // ToIntegerOrInfinity(NaN) => 0 (ES2024 §6.1.6)
                if i_f64.is_nan() {
                    0
                } else if i_f64 < 0.0 {
                    len + i_f64 as i32
                } else {
                    i_f64 as i32
                }
            } else {
                0
            };
            if idx < 0 || idx >= len {
                return value::encode_undefined();
            }
            read_array_elem(&mut caller, ptr, idx as u32).unwrap_or(value::encode_undefined())
        },
    );
    linker.define(&mut store, "env", "arr_proto_at", arr_proto_at_fn)?;

    // ── arr_proto_copy_within (#63) ──────────────────────────────────
    let arr_proto_copy_within_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return this_val;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            // target
            let raw_target = if args_count > 0 {
                let t = value::decode_f64(read_shadow_arg(&mut caller, args_base, 0));
                if t.is_nan() { 0 } else { t as i32 }
            } else {
                0
            };
            let target = if raw_target < 0 {
                (len + raw_target).max(0)
            } else {
                raw_target.min(len)
            };
            // start
            let raw_start = if args_count > 1 {
                let s = value::decode_f64(read_shadow_arg(&mut caller, args_base, 1));
                if s.is_nan() { 0 } else { s as i32 }
            } else {
                0
            };
            let start = if raw_start < 0 {
                (len + raw_start).max(0)
            } else {
                raw_start.min(len)
            };
            // end
            let raw_end = if args_count > 2 {
                let e = value::decode_f64(read_shadow_arg(&mut caller, args_base, 2));
                if e.is_nan() { len } else { e as i32 }
            } else {
                len
            };
            let end = if raw_end < 0 {
                (len + raw_end).max(0)
            } else {
                raw_end.min(len)
            };
            let count = (end - start).min(len - target).max(0) as u32;
            // 复制元素（处理重叠：从后往前复制；源为 hole 时目标也为 hole）
            if target < start {
                for i in 0..count {
                    let from = (start as u32) + i;
                    let to = (target as u32) + i;
                    if array_elem_present(&mut caller, ptr, from) {
                        let elem = read_array_elem(&mut caller, ptr, from)
                            .unwrap_or(value::encode_undefined());
                        write_array_elem(&mut caller, ptr, to, elem);
                    } else {
                        write_array_hole(&mut caller, ptr, to);
                    }
                }
            } else {
                for i in (0..count).rev() {
                    let from = (start as u32) + i;
                    let to = (target as u32) + i;
                    if array_elem_present(&mut caller, ptr, from) {
                        let elem = read_array_elem(&mut caller, ptr, from)
                            .unwrap_or(value::encode_undefined());
                        write_array_elem(&mut caller, ptr, to, elem);
                    } else {
                        write_array_hole(&mut caller, ptr, to);
                    }
                }
            }
            this_val
        },
    );
    linker.define(
        &mut store,
        "env",
        "arr_proto_copy_within",
        arr_proto_copy_within_fn,
    )?;

    // ── arr_proto_splice (#74) ───────────────────────────────────────
    let arr_proto_splice_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            // 读取 start
            let raw_start = if args_count > 0 {
                let s = value::decode_f64(read_shadow_arg(&mut caller, args_base, 0));
                if s.is_nan() { 0 } else { s as i32 }
            } else {
                0
            };
            let start_idx = if raw_start < 0 {
                (len + raw_start).max(0)
            } else {
                raw_start.min(len)
            };
            // 读取 deleteCount
            let delete_count = if args_count > 1 {
                let d = value::decode_f64(read_shadow_arg(&mut caller, args_base, 1));
                if d.is_nan() { 0 } else { (d as i32).max(0) }
            } else {
                (len - start_idx).max(0)
            };
            let actual_delete = delete_count.min(len - start_idx);
            let insert_count = (args_count - 2).max(0);
            let new_len = len - actual_delete + insert_count;
            if new_len < 0 || new_len as u64 > u64::from(MAX_ARRAY_LENGTH) {
                return make_range_error_exception(&mut caller, ARRAY_LENGTH_RANGE_ERROR);
            }
            let cap = read_array_capacity(&mut caller, ptr).unwrap_or(0) as i32;
            let mut ptr = ptr;
            if new_len > cap {
                let new_len_u32 = new_len as u32;
                let cap_u32 = cap.max(0) as u32;
                let Some(needed) = array_grow_capacity_u32(cap_u32, new_len_u32) else {
                    return make_range_error_exception(&mut caller, ARRAY_LENGTH_RANGE_ERROR);
                };
                if let Some(new_ptr) = grow_array(&mut caller, ptr, this_val, needed) {
                    ptr = new_ptr;
                } else {
                    return value::encode_undefined();
                }
            }
            // 收集被删除的元素
            let deleted_arr = array_species_create(&mut caller, this_val, actual_delete as u32);
            let Some(deleted_ptr) = resolve_array_ptr(&mut caller, deleted_arr) else {
                return value::encode_undefined();
            };
            for i in 0..actual_delete {
                let elem = read_array_elem(&mut caller, ptr, (start_idx as u32) + i as u32)
                    .unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, deleted_ptr, i as u32, elem);
            }
            write_array_length(&mut caller, deleted_ptr, actual_delete as u32);
            // 移动元素 — 遵循 ES2024 §23.1.3.31
            if insert_count != actual_delete {
                if insert_count < actual_delete {
                    // 左移 (§23.1.3.31 step 13): k 从 actualStart 递增至 len - actualDeleteCount - 1
                    for k in start_idx..(len - actual_delete) {
                        let from = k + actual_delete;
                        let to = k + insert_count;
                        let elem = read_array_elem(&mut caller, ptr, from as u32)
                            .unwrap_or(value::encode_undefined());
                        write_array_elem(&mut caller, ptr, to as u32, elem);
                    }
                } else {
                    // 右移 (§23.1.3.31 step 14): k 从 len - actualDeleteCount 递减至 actualStart + 1
                    let mut k = len - actual_delete;
                    while k > start_idx {
                        let from = k + actual_delete - 1;
                        let to = k + insert_count - 1;
                        let elem = read_array_elem(&mut caller, ptr, from as u32)
                            .unwrap_or(value::encode_undefined());
                        write_array_elem(&mut caller, ptr, to as u32, elem);
                        k -= 1;
                    }
                }
            }
            // 插入新元素
            for i in 0..insert_count {
                let item = read_shadow_arg(&mut caller, args_base, 2 + i as u32);
                write_array_elem(&mut caller, ptr, (start_idx as u32) + i as u32, item);
            }
            write_array_length(&mut caller, ptr, new_len as u32);
            deleted_arr
        },
    );
    linker.define(&mut store, "env", "arr_proto_splice", arr_proto_splice_fn)?;

    // ── arr_proto_last_index_of (ES2024 §23.1.3.20) ──────────────────
    let arr_proto_last_index_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let search = read_shadow_arg(&mut caller, args_base, 0);
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_f64(-1.0);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let from_index = if args_count > 1 {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_f64((len as i64 - 1) as f64)
            };
            array_last_index_of_from(&mut caller, ptr, len, search, from_index)
        },
    );
    linker.define(
        &mut store,
        "env",
        "arr_proto_last_index_of",
        arr_proto_last_index_of_fn,
    )?;

    // ── arr_proto_to_reversed (ES2024 §23.1.3.33) ────────────────────
    let arr_proto_to_reversed_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         _args_base: i32,
         _args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let new_arr = alloc_array(&mut caller, len);
            let Some(new_ptr) = resolve_array_ptr(&mut caller, new_arr) else {
                return value::encode_undefined();
            };
            // 结果为稠密数组：空洞按 undefined 读取。
            for i in 0..len {
                let elem = read_array_elem(&mut caller, ptr, len - 1 - i)
                    .unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, new_ptr, i, elem);
            }
            write_array_length(&mut caller, new_ptr, len);
            new_arr
        },
    );
    linker.define(
        &mut store,
        "env",
        "arr_proto_to_reversed",
        arr_proto_to_reversed_fn,
    )?;

    // ── arr_proto_to_spliced (ES2024 §23.1.3.35) ─────────────────────
    let arr_proto_to_spliced_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i64;
            // start（ToIntegerOrInfinity + 负数回绕 + 截断）
            let raw_start = if args_count > 0 {
                let s = value::decode_f64(read_shadow_arg(&mut caller, args_base, 0));
                if s.is_nan() {
                    0
                } else if s == f64::NEG_INFINITY {
                    i64::MIN
                } else if s == f64::INFINITY {
                    len
                } else {
                    s.trunc() as i64
                }
            } else {
                0
            };
            let start_idx = if raw_start < 0 {
                (len + raw_start).max(0)
            } else {
                raw_start.min(len)
            };
            // skipCount（deleteCount）
            let skip = if args_count == 0 {
                0
            } else if args_count == 1 {
                len - start_idx
            } else {
                let d = value::decode_f64(read_shadow_arg(&mut caller, args_base, 1));
                let d = if d.is_nan() { 0 } else { d.trunc() as i64 };
                d.max(0).min(len - start_idx)
            };
            let insert_count = (args_count as i64 - 2).max(0);
            let new_len = len - skip + insert_count;
            if new_len as u64 > u64::from(MAX_ARRAY_LENGTH) {
                return make_range_error_exception(&mut caller, ARRAY_LENGTH_RANGE_ERROR);
            }
            let new_arr = alloc_array(&mut caller, new_len as u32);
            let Some(new_ptr) = resolve_array_ptr(&mut caller, new_arr) else {
                return value::encode_undefined();
            };
            let mut write_idx: u32 = 0;
            // [0, start) 原样复制
            for i in 0..start_idx {
                let elem = read_array_elem(&mut caller, ptr, i as u32)
                    .unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, new_ptr, write_idx, elem);
                write_idx += 1;
            }
            // 插入新元素
            for j in 0..insert_count {
                let item = read_shadow_arg(&mut caller, args_base, 2 + j as u32);
                write_array_elem(&mut caller, new_ptr, write_idx, item);
                write_idx += 1;
            }
            // [start+skip, len) 复制
            for i in (start_idx + skip)..len {
                let elem = read_array_elem(&mut caller, ptr, i as u32)
                    .unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, new_ptr, write_idx, elem);
                write_idx += 1;
            }
            write_array_length(&mut caller, new_ptr, new_len as u32);
            new_arr
        },
    );
    linker.define(
        &mut store,
        "env",
        "arr_proto_to_spliced",
        arr_proto_to_spliced_fn,
    )?;

    // ── arr_proto_with (ES2024 §23.1.3.39) ───────────────────────────
    let arr_proto_with_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i64;
            let raw_index = if args_count > 0 {
                let idx = value::decode_f64(read_shadow_arg(&mut caller, args_base, 0));
                if idx.is_nan() { 0 } else { idx.trunc() as i64 }
            } else {
                0
            };
            let actual_index = if raw_index >= 0 {
                raw_index
            } else {
                len + raw_index
            };
            if actual_index < 0 || actual_index >= len {
                return make_range_error_exception(&mut caller, "Invalid index");
            }
            let new_value = if args_count > 1 {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            let new_arr = alloc_array(&mut caller, len as u32);
            let Some(new_ptr) = resolve_array_ptr(&mut caller, new_arr) else {
                return value::encode_undefined();
            };
            for i in 0..len {
                let elem = if i == actual_index {
                    new_value
                } else {
                    read_array_elem(&mut caller, ptr, i as u32).unwrap_or(value::encode_undefined())
                };
                write_array_elem(&mut caller, new_ptr, i as u32, elem);
            }
            write_array_length(&mut caller, new_ptr, len as u32);
            new_arr
        },
    );
    linker.define(&mut store, "env", "arr_proto_with", arr_proto_with_fn)?;

    // ── arr_static_of (ES2024 §23.1.2.3 Array.of) ────────────────────
    let arr_static_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         _this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let count = args_count.max(0) as u32;
            let new_arr = alloc_array(&mut caller, count);
            let Some(new_ptr) = resolve_array_ptr(&mut caller, new_arr) else {
                return value::encode_undefined();
            };
            for i in 0..count {
                let arg = read_shadow_arg(&mut caller, args_base, i);
                write_array_elem(&mut caller, new_ptr, i, arg);
            }
            write_array_length(&mut caller, new_ptr, count);
            new_arr
        },
    );
    linker.define(&mut store, "env", "arr_static_of", arr_static_of_fn)?;

    // ── arr_static_is_array (#75) ──────────────────────────────────────
    let arr_static_is_array_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         _this_val: i64,
         args_base: i32,
         _args_count: i32|
         -> i64 {
            let val = read_shadow_arg(&mut caller, args_base, 0);
            value::encode_bool(value::is_array(val))
        },
    );
    linker.define(
        &mut store,
        "env",
        "arr_static_is_array",
        arr_static_is_array_fn,
    )?;

    // ── abort_shadow_stack_overflow (#76) ─────────────────────────────
    let abort_shadow_stack_overflow_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, shadow_sp: i32, args_bytes: i32, stack_end: i32| {
            let mut buffer = caller
                .data()
                .output
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            writeln!(
                &mut *buffer,
                "shadow stack overflow: sp=0x{shadow_sp:x} + {args_bytes} bytes > end=0x{stack_end:x}"
            ).ok();
            *caller
                .data()
                .runtime_error
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(format!(
                "shadow stack overflow: sp={shadow_sp} + {args_bytes} > end={stack_end}"
            ));
        },
    );
    linker.define(
        &mut store,
        "env",
        "abort_shadow_stack_overflow",
        abort_shadow_stack_overflow_fn,
    )?;

    // ── func_bind (#80): Function.prototype.bind ────────────────────────────
    let func_bind_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         func: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 { func_bind_impl(&mut caller, func, this_val, args_base, args_count) },
    );
    linker.define(&mut store, "env", "func_bind", func_bind_fn)?;

    // ── object_rest (#81): Exclude specified keys from object ───────────────
    let object_rest_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, excluded_keys: i64| -> i64 {
            object_rest_impl(&mut caller, obj, excluded_keys)
        },
    );
    linker.define(&mut store, "env", "object_rest", object_rest_fn)?;

    // ── obj_spread (#82): Copy own enumerable properties ────────────────────
    let obj_spread_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, dest: i64, source: i64| {
            obj_spread_impl(&mut caller, dest, source);
        },
    );
    linker.define(&mut store, "env", "obj_spread", obj_spread_fn)?;

    // ── Import 83: has_own_property(i64, i32) -> i64 ──────────────────────────
    let has_own_property_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_ptr: i32| -> i64 {
            if !value::is_object(obj) && !value::is_function(obj) && !value::is_array(obj) {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) =
                    Some("TypeError: hasOwnProperty called on non-object".to_string());
                return value::encode_undefined();
            }
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_bool(false);
            };
            let found = find_property_slot_by_name_id(&mut caller, ptr, key_ptr as u32);
            value::encode_bool(found.is_some())
        },
    );
    linker.define(&mut store, "env", "has_own_property", has_own_property_fn)?;
    // ── Import 88: obj_create(i64, i64) -> i64 ────────────────────────────────
    let obj_create_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, proto: i64, properties: i64| -> i64 {
            if !value::is_undefined(proto) && !value::is_null(proto) && !value::is_js_object(proto)
            {
                return make_type_error_exception(
                    &mut caller,
                    "Object.create prototype may only be an object or null",
                );
            }
            let env = match WasmEnv::from_caller(&mut caller) {
                Some(e) => e,
                None => return value::encode_undefined(),
            };
            let obj_handle = if value::is_null(proto) {
                alloc_host_null_proto_object(&mut caller, &env, 0)
            } else {
                let o = alloc_host_object(&mut caller, &env, 0);
                if !value::is_undefined(proto) {
                    let proto_handle = object_proto_handle_from_value(&mut caller, proto);
                    let _ = object_write_proto_handle(&mut caller, o, proto_handle);
                }
                o
            };
            if !object_create_apply_properties(&mut caller, obj_handle, properties) {
                return value::encode_undefined();
            }
            obj_handle
        },
    );
    linker.define(&mut store, "env", "obj_create", obj_create_fn)?;
    // ── Import 90: obj_set_proto_of(i64, i64) -> i64 ──────────────────────────
    let obj_set_proto_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, proto: i64| -> i64 {
            if !value::is_js_object(obj) {
                return obj;
            }
            if !value::is_js_object(proto) && !value::is_null(proto) {
                return make_type_error_exception(
                    &mut caller,
                    "Object.setPrototypeOf prototype must be an object or null",
                );
            }
            let new_handle = object_proto_handle_from_value(&mut caller, proto);
            let current_handle = object_read_current_proto_handle(&mut caller, obj);
            if current_handle == Some(new_handle) {
                return obj;
            }
            if !is_extensible_impl(&mut caller, obj) {
                return make_type_error_exception(
                    &mut caller,
                    "Object.setPrototypeOf: object is not extensible",
                );
            }
            if !value::is_null(proto) && value::is_js_object(proto) {
                let mut current = new_handle;
                let mut depth = 0u32;
                const MAX_PROTO_DEPTH: u32 = 1000;
                let obj_handle_raw = handle_index_of(&mut caller, obj) as u32;
                while current != 0xFFFF_FFFF && current != 0 && depth < MAX_PROTO_DEPTH {
                    if current == obj_handle_raw {
                        return make_type_error_exception(&mut caller, "Cyclic __proto__ value");
                    }
                    if current & 0x8000_0000 != 0 {
                        break; // proxy handle: 不走 obj_table，跳过
                    }
                    let Some(current_ptr) = resolve_handle_idx(&mut caller, current as usize)
                    else {
                        break;
                    };
                    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                        break;
                    };
                    let d = mem.data(&caller);
                    if current_ptr + 4 > d.len() {
                        break;
                    }
                    current = u32::from_le_bytes([
                        d[current_ptr],
                        d[current_ptr + 1],
                        d[current_ptr + 2],
                        d[current_ptr + 3],
                    ]);
                    depth += 1;
                }
            }
            let _ = object_write_proto_handle(&mut caller, obj, new_handle);
            obj
        },
    );
    linker.define(&mut store, "env", "obj_set_proto_of", obj_set_proto_of_fn)?;

    // ── Import 92: obj_is(i64, i64) -> i64 ────────────────────────────────────
    let obj_is_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, val1: i64, val2: i64| -> i64 {
            // SameValue (ECMAScript 7.2.11)
            // 注意: wjsm 使用 NaN-boxing 编码，NaN-boxed 值的高位与 IEEE NaN 重叠，
            // 必须先区分数值类型再应用 IEEE 754 语义，否则 Object.is(null, undefined) 会错误返回 true
            let bits1 = val1 as u64;
            let bits2 = val2 as u64;
            let is_f64_1 = value::is_f64(val1);
            let is_f64_2 = value::is_f64(val2);
            if is_f64_1 && is_f64_2 {
                // 两者都是 IEEE 754 数值（含 signaling NaN）
                // +0 != -0
                if bits1 == 0 && bits2 == 0x8000_0000_0000_0000 {
                    return value::encode_bool(false);
                }
                if bits1 == 0x8000_0000_0000_0000 && bits2 == 0 {
                    return value::encode_bool(false);
                }
                // NaN == NaN (signaling NaN 区域)
                let f1 = value::decode_f64(val1);
                let f2 = value::decode_f64(val2);
                if f1.is_nan() && f2.is_nan() {
                    return value::encode_bool(true);
                }
                value::encode_bool(bits1 == bits2)
            } else {
                // 至少一个是 NaN-boxed JS 值（或 canonical quiet NaN）
                // NaN-boxed 值用 bitwise 比较：不同 handle/index 表示不同对象
                value::encode_bool(bits1 == bits2)
            }
        },
    );
    linker.define(&mut store, "env", "obj_is", obj_is_fn)?;
    // ── Import 93: obj_proto_to_string(i64) -> i64 ────────────────────────────
    let obj_proto_to_string_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            obj_proto_to_string_impl(&mut caller, obj)
        },
    );
    linker.define(
        &mut store,
        "env",
        "obj_proto_to_string",
        obj_proto_to_string_fn,
    )?;
    // ── Import 94: obj_proto_value_of(i64) -> i64 ─────────────────────────────
    let obj_proto_value_of_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, obj: i64| -> i64 { obj },
    );
    linker.define(
        &mut store,
        "env",
        "obj_proto_value_of",
        obj_proto_value_of_fn,
    )?;
    let obj_proto_init_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let to_string =
                create_native_callable(caller.data(), NativeCallable::ObjectProtoToString);
            let value_of =
                create_native_callable(caller.data(), NativeCallable::ObjectProtoValueOf);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "toString", to_string);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "valueOf", value_of);
            value::encode_undefined()
        },
    );
    linker.define(&mut store, "env", "obj_proto_init", obj_proto_init_fn)?;

    // ═══════════════════════════════════════════════════════════════════
    // ── BigInt host functions ──────────────────────────────────────────
    // ═══════════════════════════════════════════════════════════════════

    // ── Import 95: bigint_from_literal(i32, i32) → i64 ─────────────────
    Ok(())
}
