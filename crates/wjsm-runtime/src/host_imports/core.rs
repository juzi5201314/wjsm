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

pub(crate) fn op_in_impl(caller: &mut Caller<'_, RuntimeState>, object: i64, prop: i64) -> i64 {
    // 检查 object 是否有 prop 属性
    if !value::is_object(object) && !value::is_function(object) {
        *caller
            .data()
            .runtime_error
            .lock()
            .expect("runtime error mutex") =
            Some("TypeError: cannot use 'in' operator on non-object".to_string());
        return value::encode_bool(false);
    }

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
        let f = f64::from_bits(prop as u64);
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
            let af = f64::from_bits(a as u64);
            let bf = f64::from_bits(b as u64);
            let result = af - bf * (af / bf).trunc();
            result.to_bits() as i64
        },
    );
    linker.define(&mut store, "env", "f64_mod", f)?;

    // ── Import 2: f64_pow(i64, i64) → i64 ───────────────────────────────
    let f = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let af = f64::from_bits(a as u64);
            let bf = f64::from_bits(b as u64);
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

    // ── Import 4: iterator_from(i64) → i64 ──────────────────────────────
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            if let Some(string_data) = read_value_string_bytes(&mut caller, val) {
                let mut iters = caller.data().iterators.lock().expect("iterators mutex");
                let handle = iters.len() as u32;
                iters.push(IteratorState::StringIter {
                    data: string_data,
                    byte_pos: 0,
                });
                return value::encode_handle(value::TAG_ITERATOR, handle);
            }

            if value::is_array(val)
                && let Some(ptr) = resolve_handle(&mut caller, val)
            {
                let length = read_array_length(&mut caller, ptr).unwrap_or(0);
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
                && let Some(ptr) = resolve_handle(&mut caller, val)
                && let Some(next) = read_object_property_by_name(&mut caller, ptr, "next")
                && value::is_callable(next)
            {
                let return_method = read_object_property_by_name(&mut caller, ptr, "return")
                    .filter(|candidate| value::is_callable(*candidate));
                let mut iters = caller.data().iterators.lock().expect("iterators mutex");
                let handle = iters.len() as u32;
                iters.push(IteratorState::ObjectIter {
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
        },
    );
    linker.define(&mut store, "env", "iterator_from", f)?;


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
            if let Some((slot_offset, _old_flags, _old_val)) = found {
                // 更新已有属性
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return;
                };
                let data = memory.data_mut(&mut caller);
                data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
                data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
                data[slot_offset + 16..slot_offset + 24].copy_from_slice(&getter.to_le_bytes());
                data[slot_offset + 24..slot_offset + 32].copy_from_slice(&setter.to_le_bytes());
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
                    let b_f64 = f64::from_bits(y as u64);
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
                    let a_f64 = f64::from_bits(x as u64);
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
            let af = f64::from_bits(na as u64);
            let bf = f64::from_bits(nb as u64);
            if af.is_nan() || bf.is_nan() {
                return value::encode_bool(false);
            }

            // 5. 否则 → px < py 的数值比较
            value::encode_bool(af < bf)
        },
    );
    linker.define(&mut store, "env", "abstract_compare", f)?;

    // ── Import 21: gc_collect(i32) → i32 ─────────────────────────────────────
    // 标记-清除 GC：尝试回收足够空间满足 requested_size。
    // 返回新的 heap_ptr 或 0（失败）。
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, requested_size: i32| -> i32 {
            // 获取全局变量
            let heap_ptr = {
                let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
                    return 0;
                };
                g.get(&mut caller).i32().unwrap_or(0)
            };
            let obj_table_ptr = {
                let Some(Extern::Global(g)) = caller.get_export("__obj_table_ptr") else {
                    return 0;
                };
                g.get(&mut caller).i32().unwrap_or(0)
            };
            let obj_table_count = {
                let Some(Extern::Global(g)) = caller.get_export("__obj_table_count") else {
                    return 0;
                };
                g.get(&mut caller).i32().unwrap_or(0)
            };
            let object_heap_start = {
                let Some(Extern::Global(g)) = caller.get_export("__object_heap_start") else {
                    return 0;
                };
                g.get(&mut caller).i32().unwrap_or(0)
            };
            let num_ir_functions = {
                let Some(Extern::Global(g)) = caller.get_export("__num_ir_functions") else {
                    return 0;
                };
                g.get(&mut caller).i32().unwrap_or(0)
            };
            let shadow_sp = {
                let Some(Extern::Global(g)) = caller.get_export("__shadow_sp") else {
                    return 0;
                };
                g.get(&mut caller).i32().unwrap_or(0)
            };

            // 初始化/清除标记位图（在获取内存之前）
            {
                let mut mark_bits = caller
                    .data()
                    .gc_mark_bits
                    .lock()
                    .expect("gc_mark_bits mutex");
                let needed_words = (obj_table_count as usize).div_ceil(64).max(mark_bits.len());
                if mark_bits.len() < needed_words {
                    mark_bits.resize(needed_words, 0);
                } else {
                    mark_bits.fill(0);
                }
            }

            // ── 构建根集 ──
            // 从三个来源收集根对象：
            //   1. 影子栈帧（调用栈上的对象/函数引用）
            //   2. 函数属性对象（前 num_ir_functions 个句柄）
            //   3. 定时器回调
            let mut roots: Vec<(usize, usize)> = Vec::new();
            {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return 0;
                };
                let data = memory.data(&caller);

                let add_root = |handle_idx: usize, data: &[u8], roots: &mut Vec<(usize, usize)>| {
                    let slot_addr = obj_table_ptr as usize + handle_idx * 4;
                    if slot_addr + 4 <= data.len() {
                        let obj_ptr = u32::from_le_bytes([
                            data[slot_addr],
                            data[slot_addr + 1],
                            data[slot_addr + 2],
                            data[slot_addr + 3],
                        ]) as usize;
                        if obj_ptr != 0 {
                            roots.push((handle_idx, obj_ptr));
                        }
                    }
                };

                // 3a. 影子栈：从 shadow_stack_base 扫描到 shadow_sp
                // shadow_sp 是栈指针，影子栈在 shadow_stack_base 处，每帧 8 字节
                let shadow_stack_base = object_heap_start as usize - SHADOW_STACK_SIZE as usize;
                let shadow_sp_usize = shadow_sp as usize;
                if shadow_sp_usize > shadow_stack_base {
                    let frame_count = (shadow_sp_usize - shadow_stack_base) / 8;
                    for frame in 0..frame_count {
                        let frame_addr = shadow_stack_base + frame * 8;
                        if frame_addr + 8 <= data.len() {
                            let val = i64::from_le_bytes([
                                data[frame_addr],
                                data[frame_addr + 1],
                                data[frame_addr + 2],
                                data[frame_addr + 3],
                                data[frame_addr + 4],
                                data[frame_addr + 5],
                                data[frame_addr + 6],
                                data[frame_addr + 7],
                            ]);
                            if value::is_object(val) {
                                let handle_idx = (val as u64 & 0xFFFF_FFFF) as usize;
                                add_root(handle_idx, data, &mut roots);
                            } else if value::is_function(val) {
                                // Functions are stored in handle table too
                                let func_idx = (val as u64 & 0xFFFF_FFFF) as usize;
                                if func_idx < num_ir_functions as usize {
                                    add_root(func_idx, data, &mut roots);
                                }
                            } else if value::is_closure(val) {
                                // 闭包值的 env_obj 可能包含对象引用
                                let closure_idx = value::decode_closure_idx(val) as usize;
                                let closures =
                                    caller.data().closures.lock().expect("closures mutex");
                                if let Some(entry) = closures.get(closure_idx)
                                    && value::is_object(entry.env_obj)
                                {
                                    let handle_idx =
                                        value::decode_object_handle(entry.env_obj) as usize;
                                    add_root(handle_idx, data, &mut roots);
                                }
                            }
                        }
                    }
                }

                // 3b. 函数属性对象（前 num_ir_functions 个条目）始终标记
                for handle_idx in 0..num_ir_functions as usize {
                    add_root(handle_idx, data, &mut roots);
                }

                // 3c. 定时器回调
                {
                    let timers = caller.data().timers.lock().expect("timers mutex");
                    for timer in timers.iter() {
                        let val = timer.callback;
                        if value::is_function(val) {
                            let func_idx = (val as u64 & 0xFFFF_FFFF) as usize;
                            if func_idx < num_ir_functions as usize {
                                add_root(func_idx, data, &mut roots);
                            }
                        } else if value::is_closure(val) {
                            // 闭包回调：将 env_obj 中的对象标记为根
                            let closure_idx = value::decode_closure_idx(val) as usize;
                            let closures = caller.data().closures.lock().expect("closures mutex");
                            if let Some(entry) = closures.get(closure_idx)
                                && value::is_object(entry.env_obj)
                            {
                                let handle_idx =
                                    value::decode_object_handle(entry.env_obj) as usize;
                                add_root(handle_idx, data, &mut roots);
                            }
                        }
                    }
                }

                // 3d. 闭包表中的 env_obj
                {
                    let closures = caller.data().closures.lock().expect("closures mutex");
                    for entry in closures.iter() {
                        if value::is_object(entry.env_obj) {
                            let handle_idx = value::decode_object_handle(entry.env_obj) as usize;
                            add_root(handle_idx, data, &mut roots);
                        }
                    }
                }

                // 3e. 模块命名空间对象缓存（dynamic import 返回的命名空间对象必须保持可达）
                {
                    let cache = caller
                        .data()
                        .module_namespace_cache
                        .lock()
                        .expect("module namespace cache mutex");
                    for &val in cache.values() {
                        if value::is_object(val) {
                            let handle_idx = value::decode_object_handle(val) as usize;
                            add_root(handle_idx, data, &mut roots);
                        }
                    }
                }

                // 去重
                roots.sort();
                roots.dedup_by_key(|&mut (handle_idx, _)| handle_idx);
            } // data 借用结束

            // Phase 1: Mark - 递归标记所有可达对象
            for (handle_idx, obj_ptr) in roots {
                mark_object_recursive(
                    &mut caller,
                    handle_idx,
                    obj_ptr,
                    obj_table_ptr as usize,
                    obj_table_count as usize,
                );
            }

            // Phase 2: Sweep + Compact
            // 将存活对象移动到堆开头，更新 handle table

            // 首先获取标记位图的快照
            let mark_snapshot: Vec<u64> = {
                let mark_bits = caller
                    .data()
                    .gc_mark_bits
                    .lock()
                    .expect("gc_mark_bits mutex");
                mark_bits.clone()
            };

            // 获取内存数据的可变引用
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return 0;
            };
            let data = memory.data_mut(&mut caller);

            let heap_base = object_heap_start as usize;

            // 收集存活对象信息
            let mut live_objects: Vec<(usize, usize, usize)> = Vec::new(); // (handle_idx, old_ptr, size)
            for handle_idx in 0..obj_table_count as usize {
                let word_idx = handle_idx / 64;
                let bit_idx = handle_idx % 64;
                if word_idx < mark_snapshot.len()
                    && (mark_snapshot[word_idx] & (1u64 << bit_idx)) != 0
                {
                    // 存活对象
                    let slot_addr = obj_table_ptr as usize + handle_idx * 4;
                    if slot_addr + 4 > data.len() {
                        continue;
                    }
                    let old_ptr = u32::from_le_bytes([
                        data[slot_addr],
                        data[slot_addr + 1],
                        data[slot_addr + 2],
                        data[slot_addr + 3],
                    ]) as usize;
                    if old_ptr == 0 {
                        continue;
                    }
                    // 计算对象大小（按 heap type 选择布局）
                    if old_ptr + 16 > data.len() {
                        continue;
                    }
                    let heap_type = data[old_ptr + 4];
                    let (capacity, elem_size) = if heap_type == wjsm_ir::HEAP_TYPE_ARRAY {
                        (
                            u32::from_le_bytes([
                                data[old_ptr + 12],
                                data[old_ptr + 13],
                                data[old_ptr + 14],
                                data[old_ptr + 15],
                            ]) as usize,
                            8usize,
                        )
                    } else {
                        (
                            u32::from_le_bytes([
                                data[old_ptr + 8],
                                data[old_ptr + 9],
                                data[old_ptr + 10],
                                data[old_ptr + 11],
                            ]) as usize,
                            32usize,
                        )
                    };
                    let Some(payload_size) = capacity.checked_mul(elem_size) else {
                        continue;
                    };
                    let Some(size) = 16usize.checked_add(payload_size) else {
                        continue;
                    };
                    live_objects.push((handle_idx, old_ptr, size));
                }
            }

            // 按旧指针排序，保持内存布局顺序
            live_objects.sort_by_key(|&(_, old_ptr, _)| old_ptr);

            // 计算新的位置
            let mut current_ptr = heap_base;
            for (_, _, size) in &live_objects {
                current_ptr += size;
            }
            let new_heap_end = current_ptr;
            let freed_space = heap_ptr as usize - new_heap_end;

            // 检查是否释放了足够空间
            if freed_space < requested_size as usize {
                // 空间不足，返回失败
                return 0;
            }

            // 实际移动对象
            let mut current_ptr = heap_base;
            for &(handle_idx, old_ptr, size) in &live_objects {
                if old_ptr != current_ptr {
                    // 移动对象（使用 ptr::copy 避免重叠问题）
                    unsafe {
                        std::ptr::copy(
                            data.as_ptr().add(old_ptr),
                            data.as_mut_ptr().add(current_ptr),
                            size,
                        );
                    }
                }
                // 更新 handle table
                let slot_addr = obj_table_ptr as usize + handle_idx * 4;
                data[slot_addr..slot_addr + 4].copy_from_slice(&(current_ptr as u32).to_le_bytes());
                current_ptr += size;
            }

            // 更新 heap_ptr 全局变量
            {
                let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
                    return 0;
                };
                g.set(&mut caller, Val::I32(new_heap_end as i32)).ok();
            }

            // 重置分配计数器
            {
                let mut counter = caller
                    .data()
                    .alloc_counter
                    .lock()
                    .expect("alloc_counter mutex");
                *counter = 0;
            }

            // ── Process weak references (WeakRef + FinalizationRegistry) ──
            {
                let mark_bits = caller
                    .data()
                    .gc_mark_bits
                    .lock()
                    .expect("gc_mark_bits mutex");
                let is_marked = |handle_idx: u32| -> bool {
                    let word = (handle_idx as usize) / 64;
                    let bit = (handle_idx as usize) % 64;
                    word < mark_bits.len() && (mark_bits[word] & (1u64 << bit)) != 0
                };

                // Process WeakRef entries
                {
                    let mut wr_table = caller
                        .data()
                        .weakref_table
                        .lock()
                        .expect("weakref_table mutex");
                    for entry in wr_table.iter_mut() {
                        if entry.target_handle != 0 && !is_marked(entry.target_handle) {
                            entry.target_handle = 0;
                        }
                    }
                }

                // Process FinalizationRegistry entries
                {
                    let mut fr_table = caller
                        .data()
                        .finalization_registry_table
                        .lock()
                        .expect("fr_table mutex");
                    for entry in fr_table.iter_mut() {
                        // Skip if the FinalizationRegistry object itself was collected
                        if !is_marked(entry.object_handle) {
                            continue;
                        }
                        let mut held_values = Vec::new();
                        entry.registrations.retain(|reg| {
                            if !is_marked(reg.target_handle) {
                                held_values.push(reg.held_value);
                                false
                            } else {
                                true
                            }
                        });
                        if !held_values.is_empty() {
                            let mut pending = caller
                                .data()
                                .pending_cleanup_callbacks
                                .lock()
                                .expect("pending_cleanup_callbacks mutex");
                            pending.push((entry.callback, held_values));
                        }
                    }
                }
            } // mark_bits lock released

            // Schedule FinalizationRegistry cleanup microtasks
            {
                let mut pending = caller
                    .data()
                    .pending_cleanup_callbacks
                    .lock()
                    .expect("pending_cleanup_callbacks mutex");
                let microtasks_to_schedule: Vec<(i64, Vec<i64>)> = pending.drain(..).collect();
                drop(pending);

                let mut mq = caller
                    .data()
                    .microtask_queue
                    .lock()
                    .expect("microtask_queue mutex");
                for (callback, held_values) in microtasks_to_schedule {
                    for held_value in held_values {
                        mq.push_back(Microtask::CleanupFinalizationRegistry {
                            callback,
                            held_value,
                        });
                    }
                }
            }

            new_heap_end as i32
        },
    );
    linker.define(&mut store, "env", "gc_collect", f)?;

    // ── Import 22: console_error(i64) → () ────────────────────────────────
    // Already created above as `console_error`.

    // ── Import 27: set_timeout(i64, i64) → i64 ────────────────────────────
    Ok(())
}
