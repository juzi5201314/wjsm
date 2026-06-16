use anyhow::Result;
use wasmtime::{Caller, Linker};

use crate::*;
fn concat_operand_bytes(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Vec<u8> {
    if value::is_string(val) {
        return read_value_string_bytes(caller, val).unwrap_or_default();
    }
    if value::is_array(val) {
        return array_to_string_bytes(caller, val);
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
            .runtime_error
            .lock()
            .expect("runtime error mutex") =
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
                .runtime_strings
                .lock()
                .expect("runtime strings mutex");
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
                let mut errors = caller.data().error_table.lock().unwrap();
                errors.push(ErrorEntry {
                    name: String::new(),
                    message: String::new(),
                    value: val,
                });
            }
            let rendered = render_value(&mut caller, val).unwrap_or_else(|_| "unknown".to_string());
            let mut buffer = caller
                .data()
                .output
                .lock()
                .expect("runtime output buffer mutex should not be poisoned");
            writeln!(&mut *buffer, "Uncaught exception: {rendered}").ok();
            *caller
                .data()
                .runtime_error
                .lock()
                .expect("runtime error mutex") = Some(format!("Uncaught exception: {rendered}"));
        },
    );
    linker.define(&mut store, "env", "throw", f)?;

    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, handle: i64| -> i64 {
            let handle_idx = value::decode_handle(handle) as usize;
            let mut iters = caller.data().iterators.lock().expect("iterators mutex");
            if let Some(iter) = iters.get_mut(handle_idx) {
                match iter {
                    IteratorState::StringIter { data, byte_pos } => {
                        if *byte_pos < data.len() {
                            let ch = data[*byte_pos] as char;
                            drop(iters);
                            store_runtime_string(&caller, ch.to_string())
                        } else {
                            value::encode_undefined()
                        }
                    }
                    IteratorState::ArrayIter { ptr, index, length } => {
                        if *index < *length {
                            let idx = *index;
                            let arr_ptr = *ptr;
                            drop(iters);
                            read_array_elem(&mut caller, arr_ptr, idx)
                                .unwrap_or(value::encode_undefined())
                        } else {
                            value::encode_undefined()
                        }
                    }
                    IteratorState::MapKeyIter { keys, index } => {
                        if (*index as usize) < keys.len() {
                            keys[*index as usize]
                        } else {
                            value::encode_undefined()
                        }
                    }
                    IteratorState::MapValueIter { values, index } => {
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
                            typedarray_element_read_entry(&mut caller, &entry, idx)
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
                            let entry = alloc_array(&mut caller, 2);
                            if let Some(entry_ptr) = resolve_array_ptr(&mut caller, entry) {
                                let elem = typedarray_element_read_entry(
                                    &mut caller,
                                    &typedarray_entry,
                                    idx,
                                )
                                .unwrap_or_else(value::encode_undefined);
                                write_array_elem(
                                    &mut caller,
                                    entry_ptr,
                                    0,
                                    value::encode_f64(idx as f64),
                                );
                                write_array_elem(&mut caller, entry_ptr, 1, elem);
                                write_array_length(&mut caller, entry_ptr, 2);
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
                            .runtime_error
                            .lock()
                            .expect("runtime error mutex") =
                            Some("TypeError: value is not iterable".to_string());
                        value::encode_undefined()
                    }
                }
            } else {
                value::encode_undefined()
            }
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
                let mut enums = caller.data().enumerators.lock().expect("enumerators mutex");
                let handle = enums.len() as u32;
                enums.push(EnumeratorState::StringEnum {
                    length: len,
                    index: 0,
                });
                value::encode_handle(value::TAG_ENUMERATOR, handle)
            } else if value::is_object(val) || value::is_function(val) {
                // 对象/函数属性枚举
                let keys = enumerate_object_keys(&mut caller, val);
                let mut enums = caller.data().enumerators.lock().expect("enumerators mutex");
                let handle = enums.len() as u32;
                enums.push(EnumeratorState::ObjectEnum { keys, index: 0 });
                value::encode_handle(value::TAG_ENUMERATOR, handle)
            } else if value::is_f64(val) {
                // 数字：无枚举属性（JS 语义：for..in on number = no iteration）
                let mut enums = caller.data().enumerators.lock().expect("enumerators mutex");
                let handle = enums.len() as u32;
                enums.push(EnumeratorState::StringEnum {
                    length: 0,
                    index: 0,
                });
                value::encode_handle(value::TAG_ENUMERATOR, handle)
            } else if value::is_bool(val) {
                // 布尔值：无枚举属性（JS 语义：for..in on boolean = no iteration）
                let mut enums = caller.data().enumerators.lock().expect("enumerators mutex");
                let handle = enums.len() as u32;
                enums.push(EnumeratorState::StringEnum {
                    length: 0,
                    index: 0,
                });
                value::encode_handle(value::TAG_ENUMERATOR, handle)
            } else {
                let mut enums = caller.data().enumerators.lock().expect("enumerators mutex");
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
            let mut enums = caller.data().enumerators.lock().expect("enumerators mutex");
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
            let mut enums = caller.data().enumerators.lock().expect("enumerators mutex");
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
                            .runtime_error
                            .lock()
                            .expect("runtime error mutex") =
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
            let mut enums = caller.data().enumerators.lock().expect("enumerators mutex");
            let done = if let Some(enm) = enums.get_mut(handle_idx) {
                match enm {
                    EnumeratorState::StringEnum { length, index } => *index >= *length,
                    EnumeratorState::ObjectEnum { keys, index } => *index >= keys.len(),
                    EnumeratorState::Error => {
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .expect("runtime error mutex") =
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
                let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
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
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, value: i64, constructor: i64| -> i64 {
            // 1. 原始类型直接返回 false
            if !value::is_js_object(value) {
                return value::encode_bool(false);
            }

            // 2. 检查 constructor 是否是对象或函数或 Proxy
            if !value::is_js_object(constructor) {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: Right-hand side of instanceof is not an object".to_string());
                return value::encode_undefined();
            }

            // 3. 获取 constructor 的 "prototype" 属性
            let proto_prop = store_runtime_string(&caller, "prototype".to_string());
            let prototype_val = reflect_get_impl(&mut caller, constructor, proto_prop);

            // 4. 如果 prototype 不是对象/函数/Proxy/null，抛出 TypeError
            if !value::is_js_object(prototype_val) && !value::is_null(prototype_val) {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: Function has non-object prototype property".to_string());
                return value::encode_undefined();
            }

            let prototype = prototype_val;

            // 5. 遍历 value 的原型链
            let proto_target = match resolve_handle(&mut caller, prototype) {
                Some(p) => p as u32,
                None => return value::encode_bool(false),
            };
            let mut current_ptr = match resolve_handle(&mut caller, value) {
                Some(p) => p,
                None => return value::encode_bool(false),
            };
            loop {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return value::encode_bool(false);
                };
                let data = memory.data(&caller);
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
                // 通过 handle 表解析 proto_handle → proto_ptr
                let Some(proto_ptr) = resolve_handle_idx(&mut caller, proto_handle as usize) else {
                    return value::encode_bool(false);
                };
                if proto_ptr == proto_target as usize {
                    return value::encode_bool(true);
                }
                current_ptr = proto_ptr;
            }
        },
    );
    linker.define(&mut store, "env", "op_instanceof", f)?;

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
                let s = render_value(&mut caller, arg).unwrap_or_default();
                result.extend(s.into_bytes());
            }
            let s = String::from_utf8(result).unwrap_or_default();
            store_runtime_string(&caller, s)
        },
    );
    linker.define(&mut store, "env", "string_concat_va", f)?;

    // ── Import 18: define_property(i64, i32, i64) → () ────────────────────
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key: i32, desc: i64| {
            // 检查 obj 和 desc 是否是对象或函数
            if (!value::is_object(obj) && !value::is_function(obj) && !value::is_array(obj))
                || (!value::is_object(desc) && !value::is_function(desc) && !value::is_array(desc))
            {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: Object.defineProperty called on non-object".to_string());
                return;
            }
            let obj_ptr = match resolve_handle(&mut caller, obj) {
                Some(p) => p,
                None => return,
            };
            let desc_ptr = match resolve_handle(&mut caller, desc) {
                Some(p) => p,
                None => return,
            };
            let name_id = key as u32;
            // 读取描述符属性
            let prop_value = read_object_property_by_name(&mut caller, desc_ptr, "value");
            let prop_writable = read_object_property_by_name(&mut caller, desc_ptr, "writable");
            let prop_enumerable = read_object_property_by_name(&mut caller, desc_ptr, "enumerable");
            let prop_configurable =
                read_object_property_by_name(&mut caller, desc_ptr, "configurable");
            let prop_get = read_object_property_by_name(&mut caller, desc_ptr, "get");
            let prop_set = read_object_property_by_name(&mut caller, desc_ptr, "set");

            // 检查是否为访问器属性（有 get 或 set）
            if let Some(getter) = prop_get
                && !value::is_undefined(getter)
                && !value::is_callable(getter)
            {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: property getter must be callable".to_string());
                return;
            }
            if let Some(setter) = prop_set
                && !value::is_undefined(setter)
                && !value::is_callable(setter)
            {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: property setter must be callable".to_string());
                return;
            }

            let is_accessor = prop_get.is_some() || prop_set.is_some();

            // 检查 descriptor 冲突：accessor 和 data 字段不能共存
            // ToPropertyDescriptor: 如果同时有 get/set 和 value/writable，应抛 TypeError
            if is_accessor {
                // 访问器属性不能有 value 或 writable 字段
                if prop_value.is_some() {
                    *caller.data().runtime_error.lock().expect("runtime error mutex") =
                        Some("TypeError: Invalid property descriptor: cannot specify both accessor and value".to_string());
                    return;
                }
                if prop_writable.is_some() {
                    *caller.data().runtime_error.lock().expect("runtime error mutex") =
                        Some("TypeError: Invalid property descriptor: cannot specify both accessor and writable".to_string());
                    return;
                }
            }

            // 计算 flags: bit0=configurable, bit1=enumerable, bit2=writable, bit3=is_accessor
            // JS 规范：缺省的属性特性默认为 false
            let mut flags: i32 = 0;
            if is_accessor {
                flags |= constants::FLAG_IS_ACCESSOR; // is_accessor
            }
            if !is_accessor && prop_writable.is_some_and(|v| !value::is_falsy(v)) {
                flags |= constants::FLAG_WRITABLE; // writable (仅数据属性)
            }
            if prop_enumerable.is_some_and(|v| !value::is_falsy(v)) {
                flags |= constants::FLAG_ENUMERABLE; // enumerable
            }
            if prop_configurable.is_some_and(|v| !value::is_falsy(v)) {
                flags |= constants::FLAG_CONFIGURABLE; // configurable
            }

            let val = prop_value.unwrap_or(value::encode_undefined());
            let getter = prop_get.unwrap_or(value::encode_undefined());
            let setter = prop_set.unwrap_or(value::encode_undefined());

            // 查找已有属性
            let found = find_property_slot_by_name_id(&mut caller, obj_ptr, name_id);
            if let Some((slot_offset, old_flags, _old_val)) = found {
                // 读取旧的 getter/setter 以保留未被描述符覆盖的值
                let old_accessor = (old_flags & constants::FLAG_IS_ACCESSOR) != 0;
                let (old_getter, old_setter) = {
                    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                        return;
                    };
                    let data = memory.data(&caller);
                    if old_accessor {
                        let g = i64::from_le_bytes(
                            data[slot_offset + 16..slot_offset + 24].try_into().unwrap(),
                        );
                        let s = i64::from_le_bytes(
                            data[slot_offset + 24..slot_offset + 32].try_into().unwrap(),
                        );
                        (g, s)
                    } else {
                        (value::encode_undefined(), value::encode_undefined())
                    }
                };
                // 使用描述符值或保留旧值
                let final_getter = prop_get.unwrap_or(old_getter);
                let final_setter = prop_set.unwrap_or(old_setter);
                // 更新已有属性
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return;
                };
                let data = memory.data_mut(&mut caller);
                data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
                data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
                data[slot_offset + 16..slot_offset + 24]
                    .copy_from_slice(&final_getter.to_le_bytes());
                data[slot_offset + 24..slot_offset + 32]
                    .copy_from_slice(&final_setter.to_le_bytes());
            } else {
                // 添加新属性
                let (capacity, num_props) = {
                    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                        return;
                    };
                    let data = memory.data(&caller);
                    if obj_ptr + 16 > data.len() {
                        return;
                    }
                    let capacity = u32::from_le_bytes([
                        data[obj_ptr + 8],
                        data[obj_ptr + 9],
                        data[obj_ptr + 10],
                        data[obj_ptr + 11],
                    ]) as usize;
                    let num_props = u32::from_le_bytes([
                        data[obj_ptr + 12],
                        data[obj_ptr + 13],
                        data[obj_ptr + 14],
                        data[obj_ptr + 15],
                    ]) as usize;
                    (capacity, num_props)
                };

                // 实际写入用的对象指针（可能因扩容而改变）
                let mut actual_obj_ptr = obj_ptr;

                // 如果容量不足，执行 host 侧扩容
                if num_props >= capacity {
                    // 读取全局变量
                    let obj_table_ptr = {
                        let Some(Extern::Global(g)) = caller.get_export("__obj_table_ptr") else {
                            return;
                        };
                        g.get(&mut caller).i32().unwrap_or(0) as usize
                    };
                    let heap_ptr = {
                        let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
                            return;
                        };
                        g.get(&mut caller).i32().unwrap_or(0) as usize
                    };
                    let handle_idx = (obj as u64 & 0xFFFF_FFFF) as u32;

                    // 计算新容量和新大小
                    let new_capacity = if capacity == 0 { 1 } else { capacity * 2 };
                    let new_size = 16 + new_capacity * 32;

                    // 复制旧数据到新位置并更新元数据
                    {
                        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                            return;
                        };
                        let data = memory.data_mut(&mut caller);
                        if heap_ptr + new_size > data.len() {
                            return;
                        }

                        // 复制旧数据（header + 已有属性）
                        let old_size = 16 + num_props * 32;
                        data.copy_within(actual_obj_ptr..actual_obj_ptr + old_size, heap_ptr);

                        // 更新新对象的 capacity
                        data[heap_ptr + 8..heap_ptr + 12]
                            .copy_from_slice(&(new_capacity as u32).to_le_bytes());

                        // 更新 handle 表
                        let slot_addr = obj_table_ptr + handle_idx as usize * 4;
                        if slot_addr + 4 <= data.len() {
                            data[slot_addr..slot_addr + 4]
                                .copy_from_slice(&(heap_ptr as u32).to_le_bytes());
                        }
                    }

                    // 更新 __heap_ptr 全局变量
                    {
                        let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
                            return;
                        };
                        let _ = g.set(&mut caller, Val::I32((heap_ptr + new_size) as i32));
                    }

                    actual_obj_ptr = heap_ptr;
                }

                // 写入新属性
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return;
                };
                let data = memory.data_mut(&mut caller);
                let slot_offset = actual_obj_ptr + 16 + num_props * 32;
                if slot_offset + 32 > data.len() {
                    return;
                }
                data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
                data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
                data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
                data[slot_offset + 16..slot_offset + 24].copy_from_slice(&getter.to_le_bytes());
                data[slot_offset + 24..slot_offset + 32].copy_from_slice(&setter.to_le_bytes());
                let new_num_props = num_props + 1;
                data[actual_obj_ptr + 12..actual_obj_ptr + 16]
                    .copy_from_slice(&(new_num_props as u32).to_le_bytes());
            }
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
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") = Some(
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
                            .bigint_table
                            .lock()
                            .expect("bigint_table mutex");
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
                            .bigint_table
                            .lock()
                            .expect("bigint_table mutex");
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
                                .bigint_table
                                .lock()
                                .expect("bigint_table mutex");
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
                                .bigint_table
                                .lock()
                                .expect("bigint_table mutex");
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

    // ── Import 20: abstract_compare(i64, i64) → i64 ──────────────────────────────
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            // 实现 Abstract Relational Comparison (ECMAScript 7.2.17)
            // 返回值: true (a < b), false (a >= b 或无法比较)

            // 1. ToPrimitive(a, hint Number), ToPrimitive(b, hint Number)
            let pa = to_primitive(&mut caller, a);
            let pb = to_primitive(&mut caller, b);

            // 2. 若都是 String → 字典序比较
            if value::is_string(pa) && value::is_string(pb) {
                let a_str = get_string_value(&mut caller, pa);
                let b_str = get_string_value(&mut caller, pb);
                return value::encode_bool(a_str < b_str);
            }

            // 3. 否则 → ToNumber(px), ToNumber(py)
            let na = to_number(&mut caller, pa);
            let nb = to_number(&mut caller, pb);

            // 4. 若任一为 NaN → 返回 false
            let af = value::decode_f64(na);
            let bf = value::decode_f64(nb);
            if af.is_nan() || bf.is_nan() {
                return value::encode_bool(false);
            }

            // 5. 否则 → px < py 的数值比较
            value::encode_bool(af < bf)
        },
    );
    linker.define(&mut store, "env", "abstract_compare", f)?;

    // ── P4 GC framework: gc_alloc_slow / gc_maybe_collect / gc_take_freed_handle ──

    // gc_alloc_slow(size: i32, heap_type: i32, capacity: i32) -> i32
    //   fast-path bump 失败后的 slow-path：free list → bump → GC → grow。
    //   返回**线性内存 ptr**（仅地址；handle 注册在 WASM $obj_new/$arr_new 中完成）。
    //   真 OOM（无法分配）时返回 u32::MAX sentinel（调用方 unreachable trap）。
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, size: i32, heap_type: i32, capacity: i32| -> i32 {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return u32::MAX as i32;
            };
            let size = size.max(0) as usize;
            let heap_type = heap_type.max(0).min(255) as u8;
            let capacity = capacity.max(0) as u32;
            // 算法持有在 RuntimeState.gc_algorithm（Arc<Mutex>），经 GcContext 调用。
            // 先 clone Arc 释放 caller 不可变借用，再 lock，避免借用冲突。
            let gc_arc = caller.data().gc_algorithm.clone();
            // 1. alloc_slow（free list + bump）
            {
                let mut gc = gc_arc.lock().expect("gc_algorithm mutex");
                let mut ctx =
                    crate::runtime_gc::GcContext::new(&mut caller, memory, gc.algorithm_name());
                if let Some(ptr) = gc.alloc_slow(&mut ctx, size, heap_type, capacity) {
                    return ptr as i32;
                }
            }
            // 2. collect 后重试
            {
                let mut gc = gc_arc.lock().expect("gc_algorithm mutex");
                let mut ctx =
                    crate::runtime_gc::GcContext::new(&mut caller, memory, gc.algorithm_name());
                let mut roots = crate::runtime_gc::roots::RuntimeRoots;
                gc.collect_with_provider(&mut ctx, &mut roots as _);
            }
            {
                let mut gc = gc_arc.lock().expect("gc_algorithm mutex");
                let mut ctx =
                    crate::runtime_gc::GcContext::new(&mut caller, memory, gc.algorithm_name());
                if let Some(ptr) = gc.alloc_slow(&mut ctx, size, heap_type, capacity) {
                    return ptr as i32;
                }
            }
            // 3. grow + 重试（真 OOM 前最后手段）
            {
                let mut gc = gc_arc.lock().expect("gc_algorithm mutex");
                let mut ctx =
                    crate::runtime_gc::GcContext::new(&mut caller, memory, gc.algorithm_name());
                if ctx.grow(1).is_ok() {
                    if let Some(ptr) = gc.alloc_slow(&mut ctx, size, heap_type, capacity) {
                        return ptr as i32;
                    }
                }
            }
            // 真 OOM：返回 sentinel（u32::MAX），调用方应 unreachable trap。
            u32::MAX as i32
        },
    );
    linker.define(&mut store, "env", "gc_alloc_slow", f)?;

    // gc_maybe_collect()：proactive GC 触发。
    //   WASM fast-path 在每次 alloc 成功后调用。host 递增 alloc_counter，达 gc_threshold 时 collect。
    let f = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>| {
        let (should_collect, gc_arc) = {
            let state = caller.data();
            let mut counter = state.alloc_counter.lock().expect("alloc_counter mutex");
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
            let mut gc = gc_arc.lock().expect("gc_algorithm mutex");
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
            .handle_free_list
            .lock()
            .expect("handle_free_list mutex");
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
    if let Some(string_data) = read_value_string_bytes(caller, val) {
        let mut iters = caller.data().iterators.lock().expect("iterators mutex");
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
        let mut iters = caller.data().iterators.lock().expect("iterators mutex");
        let handle = iters.len() as u32;
        iters.push(IteratorState::ArrayIter {
            ptr,
            index: 0,
            length,
        });
        return value::encode_handle(value::TAG_ITERATOR, handle);
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
            let mut iters = caller.data().iterators.lock().expect("iterators mutex");
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
        let mut iters = caller.data().iterators.lock().expect("iterators mutex");
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

    let mut iters = caller.data().iterators.lock().expect("iterators mutex");
    let handle = iters.len() as u32;
    iters.push(IteratorState::Error);
    value::encode_handle(value::TAG_ITERATOR, handle)
}
