use wasmtime::*;
use wjsm_ir::value;

use crate::types::*;
use crate::runtime::*;

pub(crate) fn create_host_functions(store: &mut Store<RuntimeState>) -> Vec<(usize, Func)> {
    let typeof_fn = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
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
            } else if value::is_object(val) {
                value::encode_typeof_object()
            } else if value::is_iterator(val) || value::is_enumerator(val) {
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

    // ── Import 14: op_in(i64, i64) → i64 ───────────────────────────────────

    let op_in = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, object: i64, prop: i64| -> i64 {
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
                    read_string(&mut caller, ptr).unwrap_or_default()
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
            let mut ptr = match resolve_handle(&mut caller, object) {
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
                    let name_str = read_string_bytes(&mut caller, name_id);
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
                if let Some(proto_ptr) = resolve_handle_idx(&mut caller, proto_handle as usize) {
                    ptr = proto_ptr;
                } else {
                    return value::encode_bool(false);
                }
            }
        },
    );

    // ── Import 15: op_instanceof(i64, i64) ────────────────────────────

    let op_instanceof = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, value: i64, constructor: i64| -> i64 {
            // 1. 原始类型直接返回 false
            if !value::is_object(value) && !value::is_function(value) {
                return value::encode_bool(false);
            }

            // 2. 检查 constructor 是否是对象或函数
            if !value::is_object(constructor) && !value::is_function(constructor) {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: Right-hand side of instanceof is not an object".to_string());
                return value::encode_undefined();
            }

            // 3. 获取 constructor 的属性列表并查找 "prototype" 属性
            let ctor_ptr = match resolve_handle(&mut caller, constructor) {
                Some(p) => p,
                None => return value::encode_bool(false),
            };

            // 扫描 constructor 的属性查找 "prototype"
            let props: Vec<(u32, [u8; 8])> = {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return value::encode_bool(false);
                };
                let data = memory.data(&caller);
                if ctor_ptr + 16 > data.len() {
                    return value::encode_bool(false);
                }
                let num_props = u32::from_le_bytes([
                    data[ctor_ptr + 12],
                    data[ctor_ptr + 13],
                    data[ctor_ptr + 14],
                    data[ctor_ptr + 15],
                ]) as usize;

                (0..num_props)
                    .filter_map(|i| {
                        let slot_offset = ctor_ptr + 16 + i * 32;
                        if slot_offset + 32 <= data.len() {
                            let name_id = u32::from_le_bytes([
                                data[slot_offset],
                                data[slot_offset + 1],
                                data[slot_offset + 2],
                                data[slot_offset + 3],
                            ]);
                            let val_bytes = [
                                data[slot_offset + 8],
                                data[slot_offset + 9],
                                data[slot_offset + 10],
                                data[slot_offset + 11],
                                data[slot_offset + 12],
                                data[slot_offset + 13],
                                data[slot_offset + 14],
                                data[slot_offset + 15],
                            ];
                            Some((name_id, val_bytes))
                        } else {
                            None
                        }
                    })
                    .collect()
            };

            let mut prototype: Option<i64> = None;
            for (name_id, val_bytes) in &props {
                let name_str = read_string_bytes(&mut caller, *name_id);
                if name_str == b"prototype" {
                    prototype = Some(i64::from_le_bytes(*val_bytes));
                    break;
                }
            }

            // 4. 如果 prototype 不是对象，抛出 TypeError
            let prototype = match prototype {
                Some(p) if value::is_object(p) || value::is_function(p) => p,
                _ => {
                    *caller
                        .data()
                        .runtime_error
                        .lock()
                        .expect("runtime error mutex") =
                        Some("TypeError: Function has non-object prototype property".to_string());
                    return value::encode_undefined();
                }
            };

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

    // ── Import 16: string_concat(i64, i64) → i64 ──────────────────────────────

    vec![
        (13, typeof_fn),
        (14, op_in),
        (15, op_instanceof),
    ]
}
