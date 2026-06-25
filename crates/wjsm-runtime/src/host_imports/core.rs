use anyhow::Result;
use wasmtime::{Caller, Linker};
use wjsm_ir::wk_symbol;

use crate::*;
use crate::host_imports::get_method_by_name_id;


/// 根据 UTF-8 首字节返回该码点的字节长度（字符串迭代器按码点步进）。
pub(crate) fn utf8_code_unit_len(lead: u8) -> usize {
    match lead {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

/// 返回当前 `byte_pos` 处完整 UTF-8 码点对应的运行时字符串值（不推进位置）。
pub(crate) fn string_iter_current_value(
    caller: &Caller<'_, RuntimeState>,
    data: &[u8],
    byte_pos: usize,
) -> i64 {
    if byte_pos >= data.len() {
        return value::encode_undefined();
    }
    let ch_len = utf8_code_unit_len(data[byte_pos]);
    let end = (byte_pos + ch_len).min(data.len());
    let s = String::from_utf8_lossy(&data[byte_pos..end]).into_owned();
    store_runtime_string(caller, s)
}

/// 将字符串迭代器 `byte_pos` 推进到下一个码点。
pub(crate) fn string_iter_advance_byte_pos(data: &[u8], byte_pos: &mut usize) {
    if *byte_pos < data.len() {
        let ch_len = utf8_code_unit_len(data[*byte_pos]);
        *byte_pos += ch_len;
    }
}

/// ECMAScript `Object.defineProperty` / `DefineProperty`（§10.1.6.3 ValidateAndApplyPropertyDescriptor）。
/// 成功返回该对象（Object.defineProperty 的返回值）；失败返回可捕获 `TypeError`（`TAG_EXCEPTION`）。
pub(crate) fn define_property_impl(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name_id: u32,
    desc_handle: i64,
) -> i64 {
    if !value::is_object(obj) && !value::is_function(obj) && !value::is_array(obj) {
        return make_type_error_exception(
            caller,
            "Object.defineProperty called on non-object",
        );
    }
    if value::is_proxy(obj) {
        return make_type_error_exception(
            caller,
            "Object.defineProperty called on non-object",
        );
    }
    let desc = match parse_descriptor(caller, desc_handle) {
        Ok(d) => d,
        Err(msg) => return make_type_error_exception(caller, &msg),
    };
    match define_property_on_normal_object(caller, obj, name_id, &desc) {
        Ok(_) => obj,
        Err(msg) => make_type_error_exception(caller, &msg),
    }
}

fn concat_operand_bytes(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Vec<u8> {
    if value::is_string(val) {
        return read_value_string_bytes(caller, val).unwrap_or_default();
    }
    if value::is_array(val) {
        return array_to_string_bytes(caller, val);
    }
    if value::is_object(val) || value::is_callable(val) {
        let prim = to_primitive_with_hint(caller, val, ToPrimitiveHint::String);
        if value::is_exception(prim) {
            return Vec::new();
        }
        return get_string_value(caller, prim).into_bytes();
    }
    render_value(caller, val).unwrap_or_default().into_bytes()
}

fn array_to_string_bytes(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Vec<u8> {
    let Some(ptr) = resolve_handle(caller, val) else {
        return Vec::new();
    };
    let len = read_array_length(caller, ptr).unwrap_or(0);
    let mut out = Vec::new();
    for i in 0..len {
        if i != 0 {
            out.push(b',');
        }
        let Some(elem) = read_array_elem(caller, ptr, i) else {
            continue;
        };
        if value::is_undefined(elem) || value::is_null(elem) {
            continue;
        }
        out.extend(concat_operand_bytes(caller, elem));
    }
    out
}

/// `in` 操作符核心实现：检查属性是否在对象及其原型链上
/// 被 define_core_async 中的异步 op_in 通过 `use super::core::op_in_impl` 调用
pub(crate) fn op_in_impl(caller: &mut Caller<'_, RuntimeState>, object: i64, prop: i64) -> i64 {
    if !value::is_object(object) && !value::is_function(object) && !value::is_array(object) {
        *caller
            .data()
            .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) =
            Some("TypeError: cannot use 'in' operator on non-object".to_string());
        return value::encode_bool(false);
    }

    let prop_symbol_name_id = symbol_value_to_name_id(prop);
    // 获取属性名（ToPropertyKey 转换）
    let prop_str = if value::is_string(prop) {
        if value::is_runtime_string_handle(prop) {
            let handle = value::decode_runtime_string_handle(prop) as usize;
            let strings = caller
                .data()
                .runtime_strings.lock().unwrap_or_else(|e| e.into_inner());
            strings.get(handle).cloned().unwrap_or_default()
        } else {
            let ptr = value::decode_string_ptr(prop);
            read_string(caller, ptr).unwrap_or_default()
        }
    } else if value::is_f64(prop) {
        let f = value::decode_f64(prop);
        if f.is_nan() {
            String::from("NaN")
        } else if f == 0.0 {
            String::from("0")
        } else if f == f.floor() && f.is_finite() && f.abs() < 9007199254740992.0 {
            format!("{}", f as i64)
        } else {
            format!("{}", f)
        }
    } else if value::is_null(prop) {
        String::from("null")
    } else if value::is_undefined(prop) {
        String::from("undefined")
    } else if value::is_bool(prop) {
        format!("{}", value::decode_bool(prop))
    } else {
        String::new()
    };

    // 解析对象指针：通过 handle 表统一解析（支持 object 和 function）
    let mut ptr = match resolve_handle(caller, object) {
        Some(p) => p,
        None => return value::encode_bool(false),
    };

    if value::is_array(object) {
        if prop_str == "length" {
            return value::encode_bool(true);
        }
        if let Ok(index) = prop_str.parse::<u32>() {
            return value::encode_bool(array_elem_present(caller, ptr, index));
        }
        if let Some(symbol_name_id) = prop_symbol_name_id
            && crate::array_named_props::array_named_get_sync(caller, object, symbol_name_id)
                != value::encode_undefined()
        {
            return value::encode_bool(true);
        }
    }

    // 搜索属性，遍历原型链
    loop {
        // 读取对象属性
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return value::encode_bool(false);
        };
        let data = memory.data(&caller);
        if ptr + 16 > data.len() {
            return value::encode_bool(false);
        }

        let num_props = u32::from_le_bytes([
            data[ptr + 12],
            data[ptr + 13],
            data[ptr + 14],
            data[ptr + 15],
        ]) as usize;

        let name_ids: Vec<u32> = (0..num_props)
            .filter_map(|i| {
                let slot_offset = ptr + 16 + i * 32;
                if slot_offset + 4 <= data.len() {
                    Some(u32::from_le_bytes([
                        data[slot_offset],
                        data[slot_offset + 1],
                        data[slot_offset + 2],
                        data[slot_offset + 3],
                    ]))
                } else {
                    None
                }
            })
            .collect();

        let _ = data;

        for name_id in name_ids {
            if let Some(symbol_name_id) = prop_symbol_name_id {
                if name_id == symbol_name_id {
                    return value::encode_bool(true);
                }
                continue;
            }
            if is_symbol_name_id(name_id) {
                continue;
            }
            let name_str = read_string_bytes(caller, name_id);
            if name_str == prop_str.as_bytes() {
                return value::encode_bool(true);
            }
        }

        // 读取 __proto__（offset 0），遍历原型链
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return value::encode_bool(false);
        };
        let data = memory.data(&caller);
        if ptr + 4 > data.len() {
            return value::encode_bool(false);
        }
        let proto_handle =
            u32::from_le_bytes([data[ptr], data[ptr + 1], data[ptr + 2], data[ptr + 3]]);

        if proto_handle == 0xFFFF_FFFF {
            return value::encode_bool(false);
        }
        if let Some(proto_ptr) = resolve_handle_idx(caller, proto_handle as usize) {
            ptr = proto_ptr;
        } else {
            return value::encode_bool(false);
        }
    }
}

/// `IteratorValue` 宿主实现：按迭代器状态返回当前元素值
/// 被同步 `iterator_value` 与 `core_async::iterator_step_value_async` 共用
pub(crate) fn iterator_value_impl(caller: &mut Caller<'_, RuntimeState>, handle: i64) -> i64 {
    let handle_idx = value::decode_handle(handle) as usize;
    let mut iters = caller.data().iterators.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(iter) = iters.get_mut(handle_idx) {
        match iter {
            IteratorState::StringIter { data, byte_pos } => {
                if *byte_pos < data.len() {
                    let pos = *byte_pos;
                    let bytes = data.clone();
                    drop(iters);
                    string_iter_current_value(caller, &bytes, pos)
                } else {
                    value::encode_undefined()
                }
            }
            IteratorState::ArrayIter { ptr, index, length } => {
                if *index < *length {
                    let idx = *index;
                    let arr_ptr = *ptr;
                    drop(iters);
                    read_array_elem(caller, arr_ptr, idx).unwrap_or(value::encode_undefined())
                } else {
                    value::encode_undefined()
                }
            }
            IteratorState::MapKeyIter { map_handle, index } => {
                let table = caller.data().map_table.lock().unwrap_or_else(|e| e.into_inner());
                let val = if *map_handle < table.len() as u32 {
                    let entry = &table[*map_handle as usize];
                    let idx = *index as usize;
                    if idx < entry.keys.len() {
                        Some(entry.keys[idx])
                    } else {
                        None
                    }
                } else {
                    None
                };
                drop(table);
                val.unwrap_or(value::encode_undefined())
            }
            IteratorState::MapValueIter { map_handle, index } => {
                let table = caller.data().map_table.lock().unwrap_or_else(|e| e.into_inner());
                let val = if *map_handle < table.len() as u32 {
                    let entry = &table[*map_handle as usize];
                    let idx = *index as usize;
                    if idx < entry.values.len() {
                        Some(entry.values[idx])
                    } else {
                        None
                    }
                } else {
                    None
                };
                drop(table);
                val.unwrap_or(value::encode_undefined())
            }
            IteratorState::MapEntryIter { map_handle, index } => {
                let table = caller.data().map_table.lock().unwrap_or_else(|e| e.into_inner());
                let val = if *map_handle < table.len() as u32 {
                    let entry = &table[*map_handle as usize];
                    let idx = *index as usize;
                    if idx < entry.keys.len() {
                        let key = entry.keys[idx];
                        let value = entry.values[idx];
                        drop(table);
                        drop(iters);
                        let arr = alloc_array(caller, 2);
                        if let Some(arr_ptr) = resolve_array_ptr(caller, arr) {
                            write_array_elem(caller, arr_ptr, 0, key);
                            write_array_elem(caller, arr_ptr, 1, value);
                            write_array_length(caller, arr_ptr, 2);
                        }
                        arr
                    } else {
                        drop(table);
                        value::encode_undefined()
                    }
                } else {
                    drop(table);
                    value::encode_undefined()
                };
                val
            }
            IteratorState::HeadersKeyIter { headers_handle, index } => {
                let table = caller.data().headers_table.lock().unwrap_or_else(|e| e.into_inner());
                let val = if *headers_handle < table.len() as u32 {
                    let entry = &table[*headers_handle as usize];
                    let idx = *index as usize;
                    if idx < entry.pairs.len() {
                        let name = entry.pairs[idx].0.clone();
                        drop(table);
                        drop(iters);
                        store_runtime_string(caller, name)
                    } else {
                        drop(table);
                        value::encode_undefined()
                    }
                } else {
                    drop(table);
                    value::encode_undefined()
                };
                val
            }
            IteratorState::HeadersValueIter { headers_handle, index } => {
                let table = caller.data().headers_table.lock().unwrap_or_else(|e| e.into_inner());
                let val = if *headers_handle < table.len() as u32 {
                    let entry = &table[*headers_handle as usize];
                    let idx = *index as usize;
                    if idx < entry.pairs.len() {
                        let value = entry.pairs[idx].1.clone();
                        drop(table);
                        drop(iters);
                        store_runtime_string(caller, value)
                    } else {
                        drop(table);
                        value::encode_undefined()
                    }
                } else {
                    drop(table);
                    value::encode_undefined()
                };
                val
            }
            IteratorState::HeadersEntryIter { headers_handle, index } => {
                let table = caller.data().headers_table.lock().unwrap_or_else(|e| e.into_inner());
                let val = if *headers_handle < table.len() as u32 {
                    let entry = &table[*headers_handle as usize];
                    let idx = *index as usize;
                    if idx < entry.pairs.len() {
                        let name = entry.pairs[idx].0.clone();
                        let value = entry.pairs[idx].1.clone();
                        drop(table);
                        drop(iters);
                        let arr = alloc_array(caller, 2);
                        if let Some(arr_ptr) = resolve_array_ptr(caller, arr) {
                            write_array_elem(caller, arr_ptr, 0, store_runtime_string(caller, name));
                            write_array_elem(caller, arr_ptr, 1, store_runtime_string(caller, value));
                            write_array_length(caller, arr_ptr, 2);
                        }
                        arr
                    } else {
                        drop(table);
                        value::encode_undefined()
                    }
                } else {
                    drop(table);
                    value::encode_undefined()
                };
                val
            }
            IteratorState::SetValueIter { set_handle, index } => {
                let table = caller.data().set_table.lock().unwrap_or_else(|e| e.into_inner());
                let val = if *set_handle < table.len() as u32 {
                    let entry = &table[*set_handle as usize];
                    let idx = *index as usize;
                    if idx < entry.values.len() {
                        Some(entry.values[idx])
                    } else {
                        None
                    }
                } else {
                    None
                };
                drop(table);
                val.unwrap_or(value::encode_undefined())
            }
            IteratorState::IndexValueIter { values, index } => {
                if (*index as usize) < values.len() {
                    values[*index as usize]
                } else {
                    value::encode_undefined()
                }
            }
            IteratorState::TypedArrayValueIter {
                entry,
                index,
                length,
            } => {
                if *index < *length {
                    let entry = entry.clone();
                    let idx = *index;
                    drop(iters);
                    typedarray_element_read_entry(caller, &entry, idx)
                        .unwrap_or_else(value::encode_undefined)
                } else {
                    value::encode_undefined()
                }
            }
            IteratorState::TypedArrayEntryIter {
                entry,
                index,
                length,
            } => {
                if *index < *length {
                    let typedarray_entry = entry.clone();
                    let idx = *index;
                    drop(iters);
                    let entry = alloc_array(caller, 2);
                    if let Some(entry_ptr) = resolve_array_ptr(caller, entry) {
                        let elem = typedarray_element_read_entry(caller, &typedarray_entry, idx)
                            .unwrap_or_else(value::encode_undefined);
                        write_array_elem(caller, entry_ptr, 0, value::encode_f64(idx as f64));
                        write_array_elem(caller, entry_ptr, 1, elem);
                        write_array_length(caller, entry_ptr, 2);
                    }
                    entry
                } else {
                    value::encode_undefined()
                }
            }
            IteratorState::ObjectIter { current_value, .. } => *current_value,
            IteratorState::Error => {
                *caller
                    .data()
                    .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some("TypeError: value is not iterable".to_string());
                value::encode_undefined()
            }
        }
    } else {
        value::encode_undefined()
    }
}

/// ECMAScript OrdinaryHasInstance（§15.10.1.1），不含 `Symbol.hasInstance` 自定义路径。
async fn ordinary_has_instance_async(
    caller: &mut Caller<'_, RuntimeState>,
    value: i64,
    constructor: i64,
) -> i64 {
    if !value::is_js_object(value) {
        return value::encode_bool(false);
    }

    if !value::is_callable(constructor) {
        *caller
            .data()
            .runtime_error
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(
            "TypeError: Right-hand side of instanceof is not callable".to_string(),
        );
        return value::encode_undefined();
    }

    let proto_prop = store_runtime_string(caller, "prototype".to_string());
    let prototype_val = reflect_get_impl_with_receiver_async(
        caller,
        constructor,
        proto_prop,
        constructor,
    )
    .await;

    if !value::is_js_object(prototype_val) && !value::is_null(prototype_val) {
        *caller
            .data()
            .runtime_error
            .lock()
            .unwrap_or_else(|e| e.into_inner()) =
            Some("TypeError: Function has non-object prototype property".to_string());
        return value::encode_undefined();
    }

    let prototype = prototype_val;
    let proto_target = match resolve_handle(caller, prototype) {
        Some(p) => p as u32,
        None => return value::encode_bool(false),
    };
    let mut current_ptr = match resolve_handle(caller, value) {
        Some(p) => p,
        None => return value::encode_bool(false),
    };
    loop {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return value::encode_bool(false);
        };
        let data = memory.data(&*caller);
        if current_ptr + 4 > data.len() {
            return value::encode_bool(false);
        }
        let proto_handle = u32::from_le_bytes([
            data[current_ptr],
            data[current_ptr + 1],
            data[current_ptr + 2],
            data[current_ptr + 3],
        ]);

        if proto_handle == 0xFFFF_FFFF {
            return value::encode_bool(false);
        }
        let Some(proto_ptr) = resolve_handle_idx(caller, proto_handle as usize) else {
            return value::encode_bool(false);
        };
        if proto_ptr == proto_target as usize {
            return value::encode_bool(true);
        }
        current_ptr = proto_ptr;
    }
}

/// `instanceof` 操作符（§12.10.4）：优先 `Symbol.hasInstance`，否则 OrdinaryHasInstance。
async fn op_instanceof_async(
    caller: &mut Caller<'_, RuntimeState>,
    value: i64,
    constructor: i64,
) -> i64 {
    if !value::is_callable(constructor) {
        *caller
            .data()
            .runtime_error
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(
            "TypeError: Right-hand side of instanceof is not callable".to_string(),
        );
        return value::encode_undefined();
    }

    let has_instance_name_id = encode_symbol_name_id(wk_symbol::HAS_INSTANCE);
    match get_method_by_name_id(caller, constructor, has_instance_name_id) {
        Ok(Some(method)) => {
            let result = match call_wasm_callback_async(caller, method, constructor, &[value]).await
            {
                Ok(r) => r,
                Err(_) => return value::encode_undefined(),
            };
            if value::is_exception(result) {
                return result;
            }
            value::encode_bool(nanbox_to_bool(result))
        }
        Ok(None) => ordinary_has_instance_async(caller, value, constructor).await,
        Err(exc) => exc,
    }
}

pub(crate) fn define_core(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| {
            write_console_values(&mut caller, args_base, args_count, None);
        },
    );
    linker.define(&mut store, "env", "console_log", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| {
            write_console_values(&mut caller, args_base, args_count, Some("error"));
        },
    );
    linker.define(&mut store, "env", "console_error", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| {
            write_console_values(&mut caller, args_base, args_count, Some("warn"));
        },
    );
    linker.define(&mut store, "env", "console_warn", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| {
            write_console_values(&mut caller, args_base, args_count, Some("info"));
        },
    );
    linker.define(&mut store, "env", "console_info", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| {
            write_console_values(&mut caller, args_base, args_count, Some("debug"));
        },
    );
    linker.define(&mut store, "env", "console_debug", f)?;
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| {
            write_console_values(&mut caller, args_base, args_count, Some("trace"));
        },
    );
    linker.define(&mut store, "env", "console_trace", f)?;

    // ── Import 1: f64_mod(i64, i64) → i64 ───────────────────────────────
    let f = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let af = value::decode_f64(a);
            let bf = value::decode_f64(b);
            let result = af - bf * (af / bf).trunc();
            result.to_bits() as i64
        },
    );
    linker.define(&mut store, "env", "f64_mod", f)?;

    // ── Import 2: f64_pow(i64, i64) → i64 ───────────────────────────────
    let f = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let af = value::decode_f64(a);
            let bf = value::decode_f64(b);
            let result = af.powf(bf);
            result.to_bits() as i64
        },
    );
    linker.define(&mut store, "env", "f64_pow", f)?;

    // 已弃用：异常传播现在通过 create_exception (import 313) + WASM return 实现。
    // throw_fn 保留仅为兼容旧的 WASM 二进制，不再被新编译的代码调用。
    // 注意：由于 import table 索引稳定性约束，不能移除此函数。
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            // 将异常值存入 error_table，以便 eval 调用方能通过 ExceptionValue 恢复原始值
            {
                let mut errors = caller.data().error_table.lock().unwrap_or_else(|e| e.into_inner());
                errors.push(ErrorEntry {
                    name: String::new(),
                    message: String::new(),
                    value: val,
                });
            }
            let rendered = render_value(&mut caller, val).unwrap_or_else(|_| "unknown".to_string());
            let mut buffer = caller
                .data()
                .output.lock().unwrap_or_else(|e| e.into_inner());
            writeln!(&mut *buffer, "Uncaught exception: {rendered}").ok();
            *caller
                .data()
                .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("Uncaught exception: {rendered}"));
        },
    );
    linker.define(&mut store, "env", "throw", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, handle: i64| -> i64 {
            iterator_value_impl(&mut caller, handle)
        },
    );
    linker.define(&mut store, "env", "iterator_value", f)?;

    // ── Import 9: enumerator_from(i64) → i64 ────────────────────────────
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            if let Some(string_data) = read_value_string_bytes(&mut caller, val) {
                // 字符串枚举：遍历字节索引
                let len = string_data.len();
                let mut enums = caller.data().enumerators.lock().unwrap_or_else(|e| e.into_inner());
                let handle = enums.len() as u32;
                enums.push(EnumeratorState::StringEnum {
                    length: len,
                    index: 0,
                });
                value::encode_handle(value::TAG_ENUMERATOR, handle)
            } else if value::is_object(val) || value::is_function(val) {
                // 对象/函数属性枚举
                let keys = enumerate_object_keys(&mut caller, val);
                let mut enums = caller.data().enumerators.lock().unwrap_or_else(|e| e.into_inner());
                let handle = enums.len() as u32;
                enums.push(EnumeratorState::ObjectEnum { keys, index: 0 });
                value::encode_handle(value::TAG_ENUMERATOR, handle)
            } else if value::is_f64(val) {
                // 数字：无枚举属性（JS 语义：for..in on number = no iteration）
                let mut enums = caller.data().enumerators.lock().unwrap_or_else(|e| e.into_inner());
                let handle = enums.len() as u32;
                enums.push(EnumeratorState::StringEnum {
                    length: 0,
                    index: 0,
                });
                value::encode_handle(value::TAG_ENUMERATOR, handle)
            } else if value::is_bool(val) {
                // 布尔值：无枚举属性（JS 语义：for..in on boolean = no iteration）
                let mut enums = caller.data().enumerators.lock().unwrap_or_else(|e| e.into_inner());
                let handle = enums.len() as u32;
                enums.push(EnumeratorState::StringEnum {
                    length: 0,
                    index: 0,
                });
                value::encode_handle(value::TAG_ENUMERATOR, handle)
            } else {
                let mut enums = caller.data().enumerators.lock().unwrap_or_else(|e| e.into_inner());
                let handle = enums.len() as u32;
                enums.push(EnumeratorState::Error);
                value::encode_handle(value::TAG_ENUMERATOR, handle)
            }
        },
    );
    linker.define(&mut store, "env", "enumerator_from", f)?;

    // ── Import 10: enumerator_next(i64) → i64 ───────────────────────────
    let f = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, handle: i64| -> i64 {
            let handle_idx = value::decode_handle(handle) as usize;
            let mut enums = caller.data().enumerators.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(enm) = enums.get_mut(handle_idx) {
                match enm {
                    EnumeratorState::StringEnum { length, index } => {
                        if *index < *length {
                            *index += 1;
                        }
                    }
                    EnumeratorState::ObjectEnum { keys, index } => {
                        if *index < keys.len() {
                            *index += 1;
                        }
                    }
                    EnumeratorState::Error => {}
                }
            }
            value::encode_undefined()
        },
    );
    linker.define(&mut store, "env", "enumerator_next", f)?;

    // ── Import 11: enumerator_key(i64) → i64 ────────────────────────────
    let f = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, handle: i64| -> i64 {
            let handle_idx = value::decode_handle(handle) as usize;
            let mut enums = caller.data().enumerators.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(enm) = enums.get_mut(handle_idx) {
                match enm {
                    EnumeratorState::StringEnum { index, .. } => {
                        let key = index.to_string();
                        drop(enums);
                        return store_runtime_string(&caller, key);
                    }
                    EnumeratorState::ObjectEnum { keys, index } => {
                        let key = keys.get(*index).cloned().unwrap_or_default();
                        drop(enums);
                        return store_runtime_string(&caller, key);
                    }
                    EnumeratorState::Error => {
                        *caller
                            .data()
                            .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some("TypeError: value is not enumerable".to_string());
                        return value::encode_undefined();
                    }
                }
            }
            value::encode_undefined()
        },
    );
    linker.define(&mut store, "env", "enumerator_key", f)?;

    // ── Import 12: enumerator_done(i64) → i64 ───────────────────────────
    let f = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, handle: i64| -> i64 {
            let handle_idx = value::decode_handle(handle) as usize;
            let mut enums = caller.data().enumerators.lock().unwrap_or_else(|e| e.into_inner());
            let done = if let Some(enm) = enums.get_mut(handle_idx) {
                match enm {
                    EnumeratorState::StringEnum { length, index } => *index >= *length,
                    EnumeratorState::ObjectEnum { keys, index } => *index >= keys.len(),
                    EnumeratorState::Error => {
                        *caller
                            .data()
                            .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some("TypeError: value is not enumerable".to_string());
                        true
                    }
                }
            } else {
                true
            };
            value::encode_bool(done)
        },
    );
    linker.define(&mut store, "env", "enumerator_done", f)?;

    // ── Import 13: typeof(i64) → i64 ───────────────────────────────────────
    let f = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            if value::is_undefined(val) {
                value::encode_typeof_undefined()
            } else if value::is_null(val) {
                value::encode_typeof_object()
            } else if value::is_bool(val) {
                value::encode_typeof_boolean()
            } else if value::is_string(val) {
                value::encode_typeof_string()
            } else if value::is_callable(val) {
                value::encode_typeof_function()
            } else if value::is_proxy(val) {
                // Proxy: walk the chain to find ultimate non-proxy target
                let table = caller.data().proxy_table.lock().unwrap_or_else(|e| e.into_inner());
                let mut current_handle = value::decode_proxy_handle(val) as usize;
                let target_callable = loop {
                    match table.get(current_handle) {
                        Some(entry) => {
                            if value::is_callable(entry.target) {
                                break true;
                            }
                            if value::is_proxy(entry.target) {
                                current_handle = value::decode_proxy_handle(entry.target) as usize;
                                continue;
                            }
                            break false;
                        }
                        None => break false,
                    }
                };
                if target_callable {
                    value::encode_typeof_function()
                } else {
                    value::encode_typeof_object()
                }
            } else if value::is_object(val)
                || value::is_iterator(val)
                || value::is_enumerator(val)
                || value::is_array(val)
            {
                value::encode_typeof_object()
            } else if value::is_bigint(val) {
                value::encode_typeof_bigint()
            } else if value::is_symbol(val) {
                value::encode_typeof_symbol()
            } else if value::is_regexp(val) {
                value::encode_typeof_object()
            } else {
                value::encode_typeof_number()
            }
        },
    );
    linker.define(&mut store, "env", "typeof", f)?;

    // ── Import 15: op_instanceof(i64, i64) ────────────────────────────
    linker.func_wrap_async(
        "env",
        "op_instanceof",
        |mut caller: Caller<'_, RuntimeState>, (value, constructor): (i64, i64)| {
            Box::new(async move { op_instanceof_async(&mut caller, value, constructor).await })
        },
    )?;

    // ── Import 16: string_concat(i64, i64) → i64 ──────────────────────────────
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            if value::is_string(a) || value::is_string(b) {
                // 至少一个操作数是字符串 → 执行字符串连接
                let a_s = concat_operand_bytes(&mut caller, a);
                let b_s = concat_operand_bytes(&mut caller, b);
                let mut result = a_s;
                result.extend(b_s);
                let s = String::from_utf8(result).unwrap_or_default();
                store_runtime_string(&caller, s)
            } else {
                // 都不是 string → 返回 undefined 作为哨兵值，由 WASM 后端走数值加法
                value::encode_undefined()
            }
        },
    );
    linker.define(&mut store, "env", "string_concat", f)?;

    // ── Import 17: string_concat_va(i32, i32) → i64 ────────────────────────
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| -> i64 {
            let mut result = Vec::new();
            for i in 0..args_count as u32 {
                let arg = read_shadow_arg(&mut caller, args_base, i);
                result.extend(concat_operand_bytes(&mut caller, arg));
            }
            let s = String::from_utf8(result).unwrap_or_default();
            store_runtime_string(&caller, s)
        },
    );
    linker.define(&mut store, "env", "string_concat_va", f)?;

    // ── Import 18: define_property(i64, i32, i64) → i64 ────────────────────
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key: i32, desc: i64| -> i64 {
            define_property_impl(&mut caller, obj, key as u32, desc)
        },
    );
    linker.define(&mut store, "env", "define_property", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key: i32| -> i64 {
            // 检查 obj 是否是对象或函数
            if !value::is_object(obj) && !value::is_function(obj) {
                *caller
                    .data()
                    .runtime_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(
                    "TypeError: Object.getOwnPropertyDescriptor called on non-object".to_string(),
                );
                return value::encode_undefined();
            }

            let obj_ptr = match resolve_handle(&mut caller, obj) {
                Some(p) => p,
                None => return value::encode_undefined(),
            };
            let name_id = key as u32;

            // 查找属性（仅自身属性）
            let found = find_property_slot_by_name_id(&mut caller, obj_ptr, name_id);
            let Some((slot_offset, flags, _val)) = found else {
                return value::encode_undefined(); // 属性不存在
            };

            // 读取属性槽中的所有值
            let (value, getter, setter) = {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return value::encode_undefined();
                };
                let data = memory.data(&caller);
                if slot_offset + 32 > data.len() {
                    return value::encode_undefined();
                }
                let value = i64::from_le_bytes([
                    data[slot_offset + 8],
                    data[slot_offset + 9],
                    data[slot_offset + 10],
                    data[slot_offset + 11],
                    data[slot_offset + 12],
                    data[slot_offset + 13],
                    data[slot_offset + 14],
                    data[slot_offset + 15],
                ]);
                let getter = i64::from_le_bytes([
                    data[slot_offset + 16],
                    data[slot_offset + 17],
                    data[slot_offset + 18],
                    data[slot_offset + 19],
                    data[slot_offset + 20],
                    data[slot_offset + 21],
                    data[slot_offset + 22],
                    data[slot_offset + 23],
                ]);
                let setter = i64::from_le_bytes([
                    data[slot_offset + 24],
                    data[slot_offset + 25],
                    data[slot_offset + 26],
                    data[slot_offset + 27],
                    data[slot_offset + 28],
                    data[slot_offset + 29],
                    data[slot_offset + 30],
                    data[slot_offset + 31],
                ]);
                (value, getter, setter)
            };

            // 解析 flags
            let is_accessor = (flags & (1 << 3)) != 0;
            let configurable = (flags & 1) != 0;
            let enumerable = (flags & (1 << 1)) != 0;
            let writable = (flags & (1 << 2)) != 0;

            // 分配描述符对象（需要 4 个属性）

            match allocate_descriptor_object(
                &mut caller,
                is_accessor,
                value,
                writable,
                enumerable,
                configurable,
                getter,
                setter,
            ) {
                Some(h) => h,
                None => value::encode_undefined(),
            }
        },
    );
    linker.define(&mut store, "env", "get_own_prop_desc", f)?;

    // ── Import 19: abstract_eq(i64, i64) → i64 ──────────────────────────────
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            // 实现 Abstract Equality Comparison (ECMAScript 7.2.15)
            // 使用迭代而非递归来避免无限循环
            // 最多迭代 10 次防止死循环
            let mut x = a;
            let mut y = b;
            for _ in 0..10 {
                // 1. 同类型比较 → StrictEq
                if type_tag(x) == type_tag(y) {
                    return strict_eq(&mut caller, x, y);
                }

                // 2. null == undefined → true
                if value::is_null(x) && value::is_undefined(y) {
                    return value::encode_bool(true);
                }
                // 3. undefined == null → true
                if value::is_undefined(x) && value::is_null(y) {
                    return value::encode_bool(true);
                }

                // 4. Number == String → ToNumber(string) == number
                if value::is_f64(x) && value::is_string(y) {
                    y = to_number(&mut caller, y);
                    continue;
                }
                // 5. String == Number → string == ToNumber(number)
                if value::is_string(x) && value::is_f64(y) {
                    x = to_number(&mut caller, x);
                    continue;
                }

                // 6. Boolean == any → ToNumber(boolean) == any
                if value::is_bool(x) {
                    x = to_number(&mut caller, x);
                    continue;
                }
                // 7. any == Boolean → any == ToNumber(boolean)
                if value::is_bool(y) {
                    y = to_number(&mut caller, y);
                    continue;
                }

                // 8. Object == String/Number → ToPrimitive(object) == primitive
                if (value::is_object(x) || value::is_callable(x))
                    && (value::is_string(y) || value::is_f64(y))
                {
                    x = to_primitive(&mut caller, x);
                    continue;
                }
                // 9. String/Number == Object → primitive == ToPrimitive(object)
                if (value::is_string(x) || value::is_f64(x))
                    && (value::is_object(y) || value::is_callable(y))
                {
                    y = to_primitive(&mut caller, y);
                    continue;
                }

                // 10. BigInt == Number: 数学值比较 (ES §7.2.15)
                if value::is_bigint(x) && value::is_f64(y) {
                    let a_handle = value::decode_bigint_handle(x) as usize;
                    let b_f64 = value::decode_f64(y);
                    // NaN 或 ±∞ → false
                    if !b_f64.is_finite() {
                        return value::encode_bool(false);
                    }
                    // 非整数 → false (BigInt 总是整数)
                    if b_f64.fract() != 0.0 {
                        return value::encode_bool(false);
                    }
                    // 通过 f64 → BigInt 转换比较数学值
                    if let Some(bi_y) = num_traits::cast::FromPrimitive::from_f64(b_f64) {
                        let table = caller
                            .data()
                            .bigint_table.lock().unwrap_or_else(|e| e.into_inner());
                        return value::encode_bool(
                            table.get(a_handle).map(|bi| *bi == bi_y).unwrap_or(false),
                        );
                    }
                    return value::encode_bool(false);
                }
                // 11. Number == BigInt
                if value::is_f64(x) && value::is_bigint(y) {
                    let a_f64 = value::decode_f64(x);
                    let b_handle = value::decode_bigint_handle(y) as usize;
                    if !a_f64.is_finite() {
                        return value::encode_bool(false);
                    }
                    if a_f64.fract() != 0.0 {
                        return value::encode_bool(false);
                    }
                    if let Some(bi_x) = num_traits::cast::FromPrimitive::from_f64(a_f64) {
                        let table = caller
                            .data()
                            .bigint_table.lock().unwrap_or_else(|e| e.into_inner());
                        return value::encode_bool(
                            table.get(b_handle).map(|bi| *bi == bi_x).unwrap_or(false),
                        );
                    }
                    return value::encode_bool(false);
                }
                // 12. BigInt == String / String == BigInt: StringToBigInt → 比较 (ES §7.2.15)
                if value::is_bigint(x) && value::is_string(y) {
                    if let Some(bytes) = read_value_string_bytes(&mut caller, y) {
                        let s = String::from_utf8_lossy(&bytes)
                            .trim_end_matches('\0')
                            .to_string();
                        if let Ok(bi_y) = s.parse::<num_bigint::BigInt>() {
                            let a_handle = value::decode_bigint_handle(x) as usize;
                            let table = caller
                                .data()
                                .bigint_table.lock().unwrap_or_else(|e| e.into_inner());
                            return value::encode_bool(
                                table.get(a_handle).map(|bi| *bi == bi_y).unwrap_or(false),
                            );
                        }
                    }
                    return value::encode_bool(false);
                }
                if value::is_string(x) && value::is_bigint(y) {
                    if let Some(bytes) = read_value_string_bytes(&mut caller, x) {
                        let s = String::from_utf8_lossy(&bytes)
                            .trim_end_matches('\0')
                            .to_string();
                        if let Ok(bi_x) = s.parse::<num_bigint::BigInt>() {
                            let b_handle = value::decode_bigint_handle(y) as usize;
                            let table = caller
                                .data()
                                .bigint_table.lock().unwrap_or_else(|e| e.into_inner());
                            return value::encode_bool(
                                table.get(b_handle).map(|bi| *bi == bi_x).unwrap_or(false),
                            );
                        }
                    }
                    return value::encode_bool(false);
                }
                // 13. Symbol 与其他类型比较 → false
                if value::is_symbol(x) || value::is_symbol(y) {
                    return value::encode_bool(false);
                }
                // 14. 其他情况 → false
                return value::encode_bool(false);
            }
            // 迭代次数超限 → false
            value::encode_bool(false)
        },
    );
    linker.define(&mut store, "env", "abstract_eq", f)?;

    // ── strict_eq(i64, i64) → i64 — ECMAScript §7.2.16 ─────────────────────
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            strict_eq(&mut caller, a, b)
        },
    );
    linker.define(&mut store, "env", "strict_eq", f)?;

    // ── Import 20: abstract_compare(i64, i64) → i64 ──────────────────────────────
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            // 实现 Abstract Relational Comparison (ECMAScript §7.2.13)
            // 返回值: true (a < b), false (a >= b 或无法比较)

            let pa = to_primitive_with_hint(&mut caller, a, ToPrimitiveHint::Number);
            let pb = to_primitive_with_hint(&mut caller, b, ToPrimitiveHint::Number);

            // 2. 若都是 String → 字典序比较
            if value::is_string(pa) && value::is_string(pb) {
                let a_str = get_string_value(&mut caller, pa);
                let b_str = get_string_value(&mut caller, pb);
                return value::encode_bool(a_str < b_str);
            }

            // 3. 否则 → ToNumeric(px), ToNumeric(py) (§7.2.13 step 5)
            //    ToNumeric 对 BigInt 原样返回，不调用 ToNumber
            let na = to_numeric(&mut caller, pa);
            let nb = to_numeric(&mut caller, pb);

            // BigInt vs BigInt: 精确值比较 (§7.2.13 step 5f.iii)
            if value::is_bigint(na) && value::is_bigint(nb) {
                let a_handle = value::decode_bigint_handle(na) as usize;
                let b_handle = value::decode_bigint_handle(nb) as usize;
                let table = caller
                    .data()
                    .bigint_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let lt = match (table.get(a_handle), table.get(b_handle)) {
                    (Some(x), Some(y)) => x < y,
                    _ => false,
                };
                return value::encode_bool(lt);
            }

            // 混用 BigInt + Number: 比较数学值 (§7.2.13 step 5g-5m)
            if value::is_bigint(na) || value::is_bigint(nb) {
                let (bigint_val, num_val) = if value::is_bigint(na) {
                    (na, nb)
                } else {
                    (nb, na)
                };
                let nf = value::decode_f64(num_val);

                // h. NaN → undefined (false)
                if nf.is_nan() {
                    return value::encode_bool(false);
                }
                // i. -∞ < BigInt ∨ BigInt < +∞ → true
                if nf.is_infinite() {
                    // bigint < +∞: always true; bigint < -∞: always false
                    // But we need to know which side the bigint is on
                    // a < b: if bigint is on left, nx = bigint, ny = number
                    //   +∞: bigint < +∞ → true
                    //   -∞: bigint < -∞ → false
                    // a < b: if number is on left, nx = number, ny = bigint
                    //   +∞: +∞ < bigint → false
                    //   -∞: -∞ < bigint → true
                    let bigint_is_left = value::is_bigint(na);
                    if nf.is_sign_positive() {
                        return value::encode_bool(bigint_is_left);
                    } else {
                        return value::encode_bool(!bigint_is_left);
                    }
                }

                // k-m. ℝ(nBigInt) < ℝ(nNumber) (§7.2.13 step 5m)
                let big_handle = value::decode_bigint_handle(bigint_val) as usize;
                let table = caller
                    .data()
                    .bigint_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let Some(bi) = table.get(big_handle) else {
                    return value::encode_bool(false);
                };

                // 将数字转精确数学值并比较
                let num_less = number_less_than_bigint(nf, bi, value::is_bigint(na));
                return value::encode_bool(num_less);
            }

            // 纯 Number 比较
            let af = value::decode_f64(na);
            let bf = value::decode_f64(nb);
            if af.is_nan() || bf.is_nan() {
                return value::encode_bool(false);
            }
            value::encode_bool(af < bf)
        },
    );
    linker.define(&mut store, "env", "abstract_compare", f)?;

    // ── P4 GC framework: gc_alloc_slow / gc_maybe_collect / gc_take_freed_handle ──

    // gc_alloc_slow(size: i32, heap_type: i32, capacity: i32) -> i32
    //   fast-path bump 失败后的 slow-path：free list → bump → GC → grow。
    //   真 OOM（无法分配）时 host 返回 `Err` → Wasmtime trap（#117）；不再返回 sentinel。
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         size: i32,
         heap_type: i32,
         capacity: i32|
         -> wasmtime::Result<i32> {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return Err(wasmtime::Trap::AllocationTooLarge.into());
            };
            let size = size.max(0) as usize;
            let heap_type = heap_type.clamp(0, 255) as u8;
            let capacity = capacity.max(0) as u32;
            // 算法持有在 RuntimeState.gc_algorithm（Arc<Mutex>），经 GcContext 调用。
            // 先 clone Arc 释放 caller 不可变借用，再 lock，避免借用冲突。
            let gc_arc = caller.data().gc_algorithm.clone();
            // 1. alloc_slow（free list + bump）
            {
                let mut gc = gc_arc.lock().unwrap_or_else(|e| e.into_inner());
                let mut ctx =
                    crate::runtime_gc::GcContext::new(&mut caller, memory, gc.algorithm_name());
                if let Some(ptr) = gc.alloc_slow(&mut ctx, size, heap_type, capacity) {
                    return Ok(ptr as i32);
                }
            }
            // 2. collect 后重试
            {
                let mut gc = gc_arc.lock().unwrap_or_else(|e| e.into_inner());
                let mut ctx =
                    crate::runtime_gc::GcContext::new(&mut caller, memory, gc.algorithm_name());
                let mut roots = crate::runtime_gc::roots::RuntimeRoots;
                gc.collect_with_provider(&mut ctx, &mut roots as _);
            }
            {
                let mut gc = gc_arc.lock().unwrap_or_else(|e| e.into_inner());
                let mut ctx =
                    crate::runtime_gc::GcContext::new(&mut caller, memory, gc.algorithm_name());
                if let Some(ptr) = gc.alloc_slow(&mut ctx, size, heap_type, capacity) {
                    return Ok(ptr as i32);
                }
            }
            // 3. grow + 重试（真 OOM 前最后手段）
            {
                let mut gc = gc_arc.lock().unwrap_or_else(|e| e.into_inner());
                let mut ctx =
                    crate::runtime_gc::GcContext::new(&mut caller, memory, gc.algorithm_name());
                if ctx.grow(1).is_ok()
                    && let Some(ptr) = gc.alloc_slow(&mut ctx, size, heap_type, capacity)
                {
                    return Ok(ptr as i32);
                }
            }
            // 真 OOM：trap 中止执行，避免 u32::MAX/-1 被当作线性内存地址（#117）。
            Err(wasmtime::Trap::AllocationTooLarge.into())
        },
    );
    linker.define(&mut store, "env", "gc_alloc_slow", f)?;

    // gc_maybe_collect()：proactive GC 触发。
    //   WASM fast-path 在每次 alloc 成功后调用。host 递增 alloc_counter，达 gc_threshold 时 collect。
    let f = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>| {
        let (should_collect, gc_arc) = {
            let state = caller.data();
            let mut counter = state.alloc_counter.lock().unwrap_or_else(|e| e.into_inner());
            *counter += 1;
            (*counter >= state.gc_threshold, state.gc_algorithm.clone())
        };
        if !should_collect {
            return;
        }
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return;
        };
        {
            let mut gc = gc_arc.lock().unwrap_or_else(|e| e.into_inner());
            let mut ctx =
                crate::runtime_gc::GcContext::new(&mut caller, memory, gc.algorithm_name());
            let mut roots = crate::runtime_gc::roots::RuntimeRoots;
            gc.collect_with_provider(&mut ctx, &mut roots as _);
        }
        // 重置 alloc_counter（下一轮阈值窗口）。
        if let Ok(mut c) = caller.data().alloc_counter.lock() {
            *c = 0;
        }
    });
    linker.define(&mut store, "env", "gc_maybe_collect", f)?;

    // gc_take_freed_handle() -> i32：从 host handle_free_list pop 复用（fast-path take_or_alloc）。
    //   返回 handle（≥0）或 -1（空，调用方走 count++ 分支）。
    let f = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>| -> i32 {
        let mut list = caller
            .data()
            .handle_free_list.lock().unwrap_or_else(|e| e.into_inner());
        list.pop().map(|h| h as i32).unwrap_or(-1)
    });
    linker.define(&mut store, "env", "gc_take_freed_handle", f)?;

    // ── Import 22: console_error(i64) → () ────────────────────────────────
    // Already created above as `console_error`.

    // ── Import 27: set_timeout(i64, i64) → i64 ────────────────────────────
    Ok(())
}

pub(crate) async fn iterator_from_impl_async(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
) -> i64 {
    if value::is_iterator(val) {
        return val;
    }
    if let Some(string_data) = read_value_string_bytes(caller, val) {
        let mut iters = caller.data().iterators.lock().unwrap_or_else(|e| e.into_inner());
        let handle = iters.len() as u32;
        iters.push(IteratorState::StringIter {
            data: string_data,
            byte_pos: 0,
        });
        return value::encode_handle(value::TAG_ITERATOR, handle);
    }

    if value::is_array(val)
        && let Some(ptr) = resolve_handle(caller, val)
    {
        let length = read_array_length(caller, ptr).unwrap_or(0);
        let mut iters = caller.data().iterators.lock().unwrap_or_else(|e| e.into_inner());
        let handle = iters.len() as u32;
        iters.push(IteratorState::ArrayIter {
            ptr,
            index: 0,
            length,
        });
        return value::encode_handle(value::TAG_ITERATOR, handle);
    }

    // Set 快速路径：按插入顺序迭代 set_table.values
    if (value::is_object(val) || value::is_function(val))
        && let Some(ptr) = resolve_handle(caller, val)
        && let Some(sh) = read_object_property_by_name(caller, ptr, "__set_handle__")
    {
        let set_handle_u32 = value::decode_f64(sh) as u32;
        let table = caller.data().set_table.lock().unwrap_or_else(|e| e.into_inner());
        if (set_handle_u32 as usize) < table.len() {
            drop(table);
            let mut iters = caller.data().iterators.lock().unwrap_or_else(|e| e.into_inner());
            let handle = iters.len() as u32;
            iters.push(IteratorState::SetValueIter {
                set_handle: set_handle_u32,
                index: 0,
            });
            return value::encode_handle(value::TAG_ITERATOR, handle);
        }
    }

    if (value::is_object(val) || value::is_function(val))
        && let Some(ptr) = resolve_handle(caller, val)
        && let Some(method) = read_iterator_method(caller, ptr)
    {
        let iterator = call_iterable_method_async(caller, method, val).await;
        if value::is_iterator(iterator) {
            return iterator;
        }
        if (value::is_object(iterator) || value::is_function(iterator))
            && let Some(iter_ptr) = resolve_handle(caller, iterator)
            && let Some(next) = read_object_property_by_name(caller, iter_ptr, "next")
            && value::is_callable(next)
        {
            let return_method = read_object_property_by_name(caller, iter_ptr, "return")
                .filter(|candidate| value::is_callable(*candidate));
            let mut iters = caller.data().iterators.lock().unwrap_or_else(|e| e.into_inner());
            let handle = iters.len() as u32;
            iters.push(IteratorState::ObjectIter {
                iterator,
                next,
                return_method,
                current_value: value::encode_undefined(),
                has_current: false,
                done: false,
            });
            return value::encode_handle(value::TAG_ITERATOR, handle);
        }
    }

    if (value::is_object(val) || value::is_function(val))
        && let Some(ptr) = resolve_handle(caller, val)
        && let Some(next) = read_object_property_by_name(caller, ptr, "next")
        && value::is_callable(next)
    {
        let return_method = read_object_property_by_name(caller, ptr, "return")
            .filter(|candidate| value::is_callable(*candidate));
        let mut iters = caller.data().iterators.lock().unwrap_or_else(|e| e.into_inner());
        let handle = iters.len() as u32;
        iters.push(IteratorState::ObjectIter {
            iterator: val,
            next,
            return_method,
            current_value: value::encode_undefined(),
            has_current: false,
            done: false,
        });
        return value::encode_handle(value::TAG_ITERATOR, handle);
    }

    let mut iters = caller.data().iterators.lock().unwrap_or_else(|e| e.into_inner());
    let handle = iters.len() as u32;
    iters.push(IteratorState::Error);
    value::encode_handle(value::TAG_ITERATOR, handle)
}
