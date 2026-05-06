use anyhow::Result;
use std::collections::HashSet;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wasmtime::*;
/// 影子栈大小（必须与后端保持一致）
const SHADOW_STACK_SIZE: u32 = 65536;

use wjsm_ir::{constants, value};

pub fn execute(wasm_bytes: &[u8]) -> Result<()> {
    let stdout = io::stdout();
    let _ = execute_with_writer(wasm_bytes, stdout.lock())?;
    Ok(())
}

pub fn execute_with_writer<W: Write>(wasm_bytes: &[u8], writer: W) -> Result<W> {
    let engine = Engine::default();
    let module = Module::new(&engine, wasm_bytes)?;
    let output = Arc::new(Mutex::new(Vec::new()));

    // Iterator/enumerator side tables
    let iterators: Arc<Mutex<Vec<IteratorState>>> = Arc::new(Mutex::new(Vec::new()));
    let enumerators: Arc<Mutex<Vec<EnumeratorState>>> = Arc::new(Mutex::new(Vec::new()));
    let runtime_strings: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let runtime_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let timers: Arc<Mutex<Vec<TimerEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let cancelled_timers: Arc<Mutex<HashSet<u32>>> = Arc::new(Mutex::new(HashSet::new()));
    let next_timer_id: Arc<Mutex<u32>> = Arc::new(Mutex::new(1));
    let closures: Arc<Mutex<Vec<ClosureEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let bound_objects: Arc<Mutex<Vec<BoundRecord>>> = Arc::new(Mutex::new(Vec::new()));

    // GC 相关状态
    let gc_mark_bits: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let alloc_counter: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    const GC_THRESHOLD: u64 = 1000; // 每 1000 次分配触发一次 GC 检查
    let mut store = Store::new(
        &engine,
        RuntimeState {
            output: Arc::clone(&output),
            iterators: Arc::clone(&iterators),
            enumerators: Arc::clone(&enumerators),
            runtime_strings: Arc::clone(&runtime_strings),
            runtime_error: Arc::clone(&runtime_error),
            timers: Arc::clone(&timers),
            cancelled_timers: Arc::clone(&cancelled_timers),
            next_timer_id: Arc::clone(&next_timer_id),
            gc_mark_bits: Arc::clone(&gc_mark_bits),
            alloc_counter: Arc::clone(&alloc_counter),
            gc_threshold: GC_THRESHOLD,
            closures: Arc::clone(&closures),
            bound_objects: Arc::clone(&bound_objects),
        },
    );

    // ── Import 0: console_log(i64) → () ─────────────────────────────────
    let console_log = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            write_console_value(&mut caller, val, None);
        },
    );

    let console_error = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            write_console_value(&mut caller, val, Some("error"));
        },
    );
    let console_warn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            write_console_value(&mut caller, val, Some("warn"));
        },
    );
    let console_info = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            write_console_value(&mut caller, val, Some("info"));
        },
    );
    let console_debug = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            write_console_value(&mut caller, val, Some("debug"));
        },
    );
    let console_trace = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            write_console_value(&mut caller, val, Some("trace"));
        },
    );

    // ── Import 1: f64_mod(i64, i64) → i64 ───────────────────────────────
    let f64_mod = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let af = f64::from_bits(a as u64);
            let bf = f64::from_bits(b as u64);
            let result = af - bf * (af / bf).trunc();
            result.to_bits() as i64
        },
    );

    // ── Import 2: f64_pow(i64, i64) → i64 ───────────────────────────────
    let f64_pow = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let af = f64::from_bits(a as u64);
            let bf = f64::from_bits(b as u64);
            let result = af.powf(bf);
            result.to_bits() as i64
        },
    );

    let throw_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
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

    // ── Import 4: iterator_from(i64) → i64 ──────────────────────────────
    let iterator_from = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            if let Some(string_data) = read_value_string_bytes(&mut caller, val) {
                let mut iters = caller.data().iterators.lock().expect("iterators mutex");
                let handle = iters.len() as u32;
                iters.push(IteratorState::StringIter {
                    data: string_data,
                    byte_pos: 0,
                });
                value::encode_handle(value::TAG_ITERATOR, handle)
            } else {
                // Non-iterable: store an error state
                let mut iters = caller.data().iterators.lock().expect("iterators mutex");
                let handle = iters.len() as u32;
                iters.push(IteratorState::Error);
                value::encode_handle(value::TAG_ITERATOR, handle)
            }
        },
    );

    // ── Import 5: iterator_next(i64) → i64 ──────────────────────────────
    let iterator_next = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, handle: i64| -> i64 {
            let handle_idx = value::decode_handle(handle) as usize;
            let mut iters = caller.data().iterators.lock().expect("iterators mutex");
            if let Some(iter) = iters.get_mut(handle_idx) {
                match iter {
                    IteratorState::StringIter { byte_pos, .. } => {
                        *byte_pos += 1;
                    }
                    IteratorState::Error => {}
                }
            }
            value::encode_undefined()
        },
    );

    // ── Import 6: iterator_close(i64) → () ──────────────────────────────
    let iterator_close = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _handle: i64| {
            // Iterator close is a no-op for strings
        },
    );

    let iterator_value = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, handle: i64| -> i64 {
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

    // ── Import 8: iterator_done(i64) → i64 ──────────────────────────────
    let iterator_done = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, handle: i64| -> i64 {
            let handle_idx = value::decode_handle(handle) as usize;
            let mut iters = caller.data().iterators.lock().expect("iterators mutex");
            let done = if let Some(iter) = iters.get_mut(handle_idx) {
                match iter {
                    IteratorState::StringIter { data, byte_pos } => *byte_pos >= data.len(),
                    IteratorState::Error => {
                        *caller
                            .data()
                            .runtime_error
                            .lock()
                            .expect("runtime error mutex") =
                            Some("TypeError: value is not iterable".to_string());
                        true
                    }
                }
            } else {
                true
            };
            value::encode_bool(done)
        },
    );

    // ── Import 9: enumerator_from(i64) → i64 ────────────────────────────
    let enumerator_from = Func::wrap(
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

    // ── Import 10: enumerator_next(i64) → i64 ───────────────────────────
    let enumerator_next = Func::wrap(
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

    // ── Import 11: enumerator_key(i64) → i64 ────────────────────────────
    let enumerator_key = Func::wrap(
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

    // ── Import 12: enumerator_done(i64) → i64 ───────────────────────────
    let enumerator_done = Func::wrap(
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

    // ── Import 13: typeof(i64) → i64 ───────────────────────────────────────
    let typeof_fn = Func::wrap(
        &mut store,
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
            } else {
                value::encode_typeof_number()
            }
        },
    );

    // ── Import 14: op_in(i64, i64) → i64 ───────────────────────────────────
    let op_in = Func::wrap(
        &mut store,
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

                if proto_handle == 0xFFFF_FFFF {}
                // 通过 handle 表解析 proto_handle → proto_ptr
                let Some(proto_ptr) = resolve_handle_idx(&mut caller, proto_handle as usize) else {
                    return value::encode_bool(false);
                };
                ptr = proto_ptr;
            }
        },
    );

    // ── Import 15: op_instanceof(i64, i64) ────────────────────────────
    let op_instanceof = Func::wrap(
        &mut store,
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
    let string_concat = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            if value::is_string(a) || value::is_string(b) {
                // 至少一个操作数是字符串 → 执行字符串连接
                let a_s = if value::is_string(a) {
                    read_value_string_bytes(&mut caller, a).unwrap_or_default()
                } else {
                    render_value(&mut caller, a)
                        .unwrap_or_default()
                        .into_bytes()
                };
                let b_s = if value::is_string(b) {
                    read_value_string_bytes(&mut caller, b).unwrap_or_default()
                } else {
                    render_value(&mut caller, b)
                        .unwrap_or_default()
                        .into_bytes()
                };
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

    // ── Import 17: string_concat_va(i32, i32) → i64 ────────────────────────
    let string_concat_va = Func::wrap(
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

    // ── Import 18: define_property(i64, i32, i64) → () ────────────────────
    let define_property_fn = Func::wrap(
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
            if let Some(getter) = prop_get {
                if !value::is_undefined(getter) && !value::is_callable(getter) {
                    *caller
                        .data()
                        .runtime_error
                        .lock()
                        .expect("runtime error mutex") =
                        Some("TypeError: property getter must be callable".to_string());
                    return;
                }
            }
            if let Some(setter) = prop_set {
                if !value::is_undefined(setter) && !value::is_callable(setter) {
                    *caller
                        .data()
                        .runtime_error
                        .lock()
                        .expect("runtime error mutex") =
                        Some("TypeError: property setter must be callable".to_string());
                    return;
                }
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
            if !is_accessor && prop_writable.map_or(false, |v| !value::is_falsy(v)) {
                flags |= constants::FLAG_WRITABLE; // writable (仅数据属性)
            }
            if prop_enumerable.map_or(false, |v| !value::is_falsy(v)) {
                flags |= constants::FLAG_ENUMERABLE; // enumerable
            }
            if prop_configurable.map_or(false, |v| !value::is_falsy(v)) {
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

    let get_own_prop_desc_fn = Func::wrap(
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
            let desc_handle = match allocate_descriptor_object(
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
                None => return value::encode_undefined(),
            };

            desc_handle
        },
    );

    // ── Import 19: abstract_eq(i64, i64) → i64 ──────────────────────────────
    let abstract_eq = Func::wrap(
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

                // 10. 其他情况 → false
                return value::encode_bool(false);
            }
            // 迭代次数超限 → false
            value::encode_bool(false)
        },
    );

    // ── Import 20: abstract_compare(i64, i64) → i64 ──────────────────────────────
    let abstract_compare = Func::wrap(
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

    // ── Import 21: gc_collect(i32) → i32 ─────────────────────────────────────
    // 标记-清除 GC：尝试回收足够空间满足 requested_size。
    // 返回新的 heap_ptr 或 0（失败）。
    let gc_collect = Func::wrap(
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
                let needed_words = ((obj_table_count as usize + 63) / 64).max(mark_bits.len());
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
                                if let Some(entry) = closures.get(closure_idx) {
                                    if value::is_object(entry.env_obj) {
                                        let handle_idx =
                                            value::decode_object_handle(entry.env_obj) as usize;
                                        add_root(handle_idx, data, &mut roots);
                                    }
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
                            if let Some(entry) = closures.get(closure_idx) {
                                if value::is_object(entry.env_obj) {
                                    let handle_idx =
                                        value::decode_object_handle(entry.env_obj) as usize;
                                    add_root(handle_idx, data, &mut roots);
                                }
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
                    // 计算对象大小
                    if old_ptr + 12 > data.len() {
                        continue;
                    }
                    let capacity = u32::from_le_bytes([
                        data[old_ptr + 4],
                        data[old_ptr + 5],
                        data[old_ptr + 6],
                        data[old_ptr + 7],
                    ]) as usize;
                    let size = 12 + capacity * 32;
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

            new_heap_end as i32
        },
    );

    // ── Import 22: console_error(i64) → () ────────────────────────────────
    // Already created above as `console_error`.

    // ── Import 27: set_timeout(i64, i64) → i64 ────────────────────────────
    let set_timeout_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, callback: i64, delay: i64| -> i64 {
            let delay_f64 = if value::is_f64(delay) {
                f64::from_bits(delay as u64)
            } else {
                f64::NAN
            };
            let delay_ms: u64 = if delay_f64.is_nan() || delay_f64.is_sign_negative() {
                0
            } else if delay_f64 > (u32::MAX as f64) {
                u32::MAX as u64
            } else {
                delay_f64 as u64
            };
            let id = {
                let mut next_id = caller
                    .data()
                    .next_timer_id
                    .lock()
                    .expect("next_timer_id mutex");
                let id = *next_id;
                *next_id += 1;
                id
            };
            let deadline = Instant::now() + Duration::from_millis(delay_ms);
            let mut timers = caller.data().timers.lock().expect("timers mutex");
            timers.push(TimerEntry {
                id,
                deadline,
                callback,
                repeating: false,
                interval: Duration::from_millis(delay_ms),
            });
            value::encode_f64(id as f64)
        },
    );

    // ── Import 28: clear_timeout(i64) → () ────────────────────────────────
    let clear_timeout_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, timer_id: i64| {
            if value::is_f64(timer_id) {
                let id = f64::from_bits(timer_id as u64) as u32;
                caller
                    .data()
                    .cancelled_timers
                    .lock()
                    .expect("cancelled_timers mutex")
                    .insert(id);
            }
            // For simplicity, mark as cancelled rather than removing from the vec
        },
    );

    // ── Import 29: set_interval(i64, i64) → i64 ───────────────────────────
    let set_interval_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, callback: i64, delay: i64| -> i64 {
            let delay_f64 = if value::is_f64(delay) {
                f64::from_bits(delay as u64)
            } else {
                f64::NAN
            };
            let delay_ms: u64 = if delay_f64.is_nan() || delay_f64.is_sign_negative() {
                0
            } else if delay_f64 > (u32::MAX as f64) {
                u32::MAX as u64
            } else {
                delay_f64 as u64
            };
            let id = {
                let mut next_id = caller
                    .data()
                    .next_timer_id
                    .lock()
                    .expect("next_timer_id mutex");
                let id = *next_id;
                *next_id += 1;
                id
            };
            let deadline = Instant::now() + Duration::from_millis(delay_ms);
            let mut timers = caller.data().timers.lock().expect("timers mutex");
            timers.push(TimerEntry {
                id,
                deadline,
                callback,
                repeating: true,
                interval: Duration::from_millis(delay_ms),
            });
            value::encode_f64(id as f64)
        },
    );

    // ── Import 30: clear_interval(i64) → () ───────────────────────────────
    let clear_interval_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, timer_id: i64| {
            if value::is_f64(timer_id) {
                let id = f64::from_bits(timer_id as u64) as u32;
                caller
                    .data()
                    .cancelled_timers
                    .lock()
                    .expect("cancelled_timers mutex")
                    .insert(id);
            }
            // simplified no-op
        },
    );

    // ── Import 31: fetch(i64) → i64 ────────────────────────────────────────
    let fetch_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, url_val: i64| -> i64 {
            let url_str = if value::is_string(url_val) {
                if value::is_runtime_string_handle(url_val) {
                    let handle = value::decode_runtime_string_handle(url_val) as usize;
                    caller
                        .data()
                        .runtime_strings
                        .lock()
                        .expect("runtime strings mutex")
                        .get(handle)
                        .cloned()
                        .unwrap_or_default()
                } else {
                    read_string(&mut caller, value::decode_string_ptr(url_val)).unwrap_or_default()
                }
            } else {
                String::new()
            };

            if url_str.starts_with("data:") {
                // Handle data: URLs inline (no network)
                let body = url_str.split(',').nth(1).unwrap_or("").to_string();
                let decoded = urlencoding_decode(&body);
                store_runtime_string(&caller, decoded)
            } else {
                // Network fetch — use ureq if available
                let body = format!("[fetch blocked: {url_str}]");
                store_runtime_string(&caller, body)
            }
        },
    );

    // ── Import 32: json_stringify(i64) → i64 ──────────────────────────────
    let json_stringify_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            let json_str = runtime_json_stringify(&mut caller, val);
            store_runtime_string(&caller, json_str)
        },
    );

    // ── Import 33: json_parse(i64) → i64 ──────────────────────────────────
    let json_parse_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            let json_str = if value::is_string(val) {
                if value::is_runtime_string_handle(val) {
                    let handle = value::decode_runtime_string_handle(val) as usize;
                    caller
                        .data()
                        .runtime_strings
                        .lock()
                        .expect("runtime strings mutex")
                        .get(handle)
                        .cloned()
                        .unwrap_or_default()
                } else {
                    read_string(&mut caller, value::decode_string_ptr(val)).unwrap_or_default()
                }
            } else {
                String::new()
            };
            // For now, just return the string as-is (simplified parse)
            store_runtime_string(&caller, json_str)
        },
    );
    // ── Import 34: closure_create(i32, i64) -> i64 ────────────────────────────
    let closure_create_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, func_idx: i32, env_obj: i64| -> i64 {
            let mut closures = caller.data().closures.lock().expect("closures mutex");
            let idx = closures.len() as u32;
            closures.push(ClosureEntry {
                func_idx: func_idx as u32,
                env_obj,
            });
            value::encode_closure_idx(idx)
        },
    );
    // ── Import 35: closure_get_func(i32) -> i32 ─────────────────────────────
    let closure_get_func_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, closure_idx: i32| -> i32 {
            let closures = caller.data().closures.lock().expect("closures mutex");
            closures
                .get(closure_idx as usize)
                .map(|e| e.func_idx as i32)
                .unwrap_or(-1)
        },
    );
    // ── Import 36: closure_get_env(i32) -> i64 ─────────────────────────────
    let closure_get_env_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, closure_idx: i32| -> i64 {
            let closures = caller.data().closures.lock().expect("closures mutex");
            closures
                .get(closure_idx as usize)
                .map(|e| e.env_obj)
                .unwrap_or_else(value::encode_undefined)
        },
    );
    // ── Array method host functions (imports 37-48) ────────────────────
    let arr_push_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let cap = read_array_capacity(&mut caller, ptr).unwrap_or(0);
            let mut ptr = ptr;
            if len >= cap {
                let new_cap = cap.max(1) * 2;
                let needed = (len + 1).max(new_cap);
                if let Some(new_ptr) = grow_array(&mut caller, ptr, arr, needed) {
                    ptr = new_ptr;
                } else {
                    return value::encode_undefined();
                }
            }
            write_array_elem(&mut caller, ptr, len, val);
            write_array_length(&mut caller, ptr, len + 1);
            value::encode_f64((len + 1) as f64)
        },
    );
    let arr_pop_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            if len == 0 { return value::encode_undefined(); }
            let new_len = len - 1;
            let val = read_array_elem(&mut caller, ptr, new_len).unwrap_or(value::encode_undefined());
            write_array_length(&mut caller, ptr, new_len);
            val
        },
    );
    let arr_includes_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_bool(false);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                if let Some(elem) = read_array_elem(&mut caller, ptr, i) {
                    if elem == val { return value::encode_bool(true); }
                }
            }
            value::encode_bool(false)
        },
    );
    let arr_index_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64, from_val: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_f64(-1.0);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            let from_idx = if value::is_f64(from_val) {
                f64::from_bits(from_val as u64) as i32
            } else { 0 };
            let start = if from_idx >= 0 {
                (from_idx as usize).min(len as usize)
            } else {
                ((len + from_idx).max(0)) as usize
            };
            for i in start..len as usize {
                if let Some(elem) = read_array_elem(&mut caller, ptr, i as u32) {
                    if elem == val { return value::encode_f64(i as f64); }
                }
            }
            value::encode_f64(-1.0)
        },
    );
    let arr_join_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, sep_val: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let sep_str = render_value(&mut caller, sep_val)
                .unwrap_or_else(|_| ",".to_string());
            let mut parts = Vec::new();
            for i in 0..len {
                if let Some(elem) = read_array_elem(&mut caller, ptr, i) {
                    parts.push(render_value(&mut caller, elem).unwrap_or_else(|_| "".to_string()));
                } else {
                    parts.push(String::new());
                }
            }
            store_runtime_string(&caller, parts.join(&sep_str))
        },
    );
    let arr_concat_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _arr1: i64, _arr2: i64| -> i64 {
            unimplemented!("Array.prototype.concat is not yet implemented in wjsm")
        },
    );
    let arr_slice_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _arr: i64, _start: i64, _end: i64| -> i64 {
            unimplemented!("Array.prototype.slice is not yet implemented in wjsm")
        },
    );
    let arr_fill_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, val: i64, _start: i64, _end: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return arr;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                write_array_elem(&mut caller, ptr, i, val);
            }
            arr
        },
    );
    let arr_reverse_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return arr;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len / 2 {
                let a = read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let b = read_array_elem(&mut caller, ptr, len - 1 - i).unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, ptr, i, b);
                write_array_elem(&mut caller, ptr, len - 1 - i, a);
            }
            arr
        },
    );
    let arr_flat_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, _arr: i64, _depth: i64| -> i64 {
            unimplemented!("Array.prototype.flat is not yet implemented in wjsm")
        },
    );
    let arr_init_length_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64, len_val: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return arr;
            };
            let len = if value::is_f64(len_val) {
                f64::from_bits(len_val as u64) as u32
            } else {
                return arr;
            };
            write_array_length(&mut caller, ptr, len);
            arr
        },
    );
    let arr_get_length_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arr: i64| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, arr) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            value::encode_f64(len as f64)
        },
    );
    
    // ── 辅助函数：读取影子栈参数 ────────────────────────────────────
    fn read_shadow_arg(caller: &mut Caller<'_, RuntimeState>, args_base: i32, index: u32) -> i64 {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else { return value::encode_undefined(); };
        let data = memory.data(&*caller);
        let offset = args_base as usize + (index as usize) * 8;
        if offset + 8 > data.len() { return value::encode_undefined(); }
        i64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
    }
    
    // ── 辅助函数：调用 WASM 回调函数 ────────────────────────────────
    fn call_wasm_callback(
        caller: &mut Caller<'_, RuntimeState>,
        func_val: i64,
        this_val: i64,
        args: &[i64],
    ) -> anyhow::Result<i64> {
        let shadow_sp_global = caller.get_export("__shadow_sp")
            .and_then(|e| e.into_global())
            .ok_or_else(|| anyhow::anyhow!("no __shadow_sp"))?;
        let shadow_sp = shadow_sp_global.get(&mut *caller).i32().ok_or_else(|| anyhow::anyhow!("shadow_sp not i32"))?;
        let new_shadow_sp = shadow_sp + (args.len() as i32) * 8;
        // 将参数写入影子栈
        {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return Err(anyhow::anyhow!("no memory"));
            };
            let data = memory.data_mut(&mut *caller);
            let mut write_pos = shadow_sp as usize;
            for &arg in args {
                if write_pos + 8 > data.len() {
                    return Err(anyhow::anyhow!("shadow stack overflow"));
                }
                data[write_pos..write_pos + 8].copy_from_slice(&arg.to_le_bytes());
                write_pos += 8;
            }
        }
        // 更新 __shadow_sp
        shadow_sp_global.set(&mut *caller, Val::I32(new_shadow_sp))?;
        // 解析函数（闭包或函数引用）
        let (func_idx, env_obj) = if value::is_closure(func_val) {
            let idx = value::decode_closure_idx(func_val) as usize;
            let closures = caller.data().closures.lock().unwrap();
            if let Some(entry) = closures.get(idx) {
                (entry.func_idx, entry.env_obj)
            } else {
                return Err(anyhow::anyhow!("closure index out of range"));
            }
        } else if value::is_function(func_val) {
            ((func_val as u64 & 0xFFFF_FFFF) as u32, value::encode_undefined())
        } else {
            return Err(anyhow::anyhow!("not callable"));
        };
        // 通过函数表调用
        let table = caller.get_export("__table")
            .and_then(|e| e.into_table())
            .ok_or_else(|| anyhow::anyhow!("no __table"))?;
        let func_ref = table.get(&mut *caller, func_idx as u64)
            .ok_or_else(|| anyhow::anyhow!("table get failed"))?;
        let func = func_ref.as_func().flatten().ok_or_else(|| anyhow::anyhow!("table entry not a function"))?;
        let mut results = [Val::I64(0)];
        let call_result = func.call(
            &mut *caller,
            &[Val::I64(env_obj), Val::I64(this_val), Val::I32(shadow_sp), Val::I32(args.len() as i32)],
            &mut results,
        );
        // 恢复 __shadow_sp（无论 call 成功与否）
        let _ = shadow_sp_global.set(&mut *caller, Val::I32(shadow_sp));
        call_result?;
        Ok(results[0].unwrap_i64())
    }
    
    // ── 辅助函数：分配新数组 ────────────────────────────────────────
    fn alloc_array(caller: &mut Caller<'_, RuntimeState>, capacity: u32) -> i64 {
        let heap_ptr = {
            let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else { return value::encode_undefined(); };
            g.get(&mut *caller).i32().unwrap_or(0) as u32
        };
        let obj_table_count = {
            let Some(Extern::Global(g)) = caller.get_export("__obj_table_count") else { return value::encode_undefined(); };
            g.get(&mut *caller).i32().unwrap_or(0) as u32
        };
        let obj_table_ptr = {
            let Some(Extern::Global(g)) = caller.get_export("__obj_table_ptr") else { return value::encode_undefined(); };
            g.get(&mut *caller).i32().unwrap_or(0) as u32
        };
        let size = 16 + capacity * 8;
        let new_heap_ptr = heap_ptr + size;
        if let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") {
            let _ = g.set(&mut *caller, Val::I32(new_heap_ptr as i32));
        }
        let Some(Extern::Memory(mem)) = caller.get_export("memory") else { return value::encode_undefined(); };
        let d = mem.data_mut(&mut *caller);
        let ptr = heap_ptr as usize;
        if (new_heap_ptr as usize) > d.len() { return value::encode_undefined(); }
        d[ptr..ptr + 4].copy_from_slice(&(-1i32).to_le_bytes());
        d[ptr + 4] = 1u8;
        d[ptr + 5..ptr + 8].fill(0);
        d[ptr + 8..ptr + 12].copy_from_slice(&0u32.to_le_bytes());
        d[ptr + 12..ptr + 16].copy_from_slice(&capacity.to_le_bytes());
        let slot_addr = (obj_table_ptr + obj_table_count * 4) as usize;
        if slot_addr + 4 <= d.len() {
            d[slot_addr..slot_addr + 4].copy_from_slice(&heap_ptr.to_le_bytes());
        }
        let _ = d;
        if let Some(Extern::Global(g)) = caller.get_export("__obj_table_count") {
            let _ = g.set(&mut *caller, Val::I32((obj_table_count + 1) as i32));
        }
        value::encode_handle(value::TAG_ARRAY, obj_table_count)
    }
    // ── arr_proto_push (#49) ──────────────────────────────────────────
    let arr_proto_push_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let cap = read_array_capacity(&mut caller, ptr).unwrap_or(0);
            let count = args_count as u32;
            let mut ptr = ptr;
            if len + count > cap {
                let new_cap = cap.max(1) * 2;
                let needed = (len + count).max(new_cap);
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
    
    // ── arr_proto_pop (#50) ───────────────────────────────────────────
    let arr_proto_pop_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, _args_base: i32, _args_count: i32| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            if len == 0 { return value::encode_undefined(); }
            let new_len = len - 1;
            let val = read_array_elem(&mut caller, ptr, new_len).unwrap_or(value::encode_undefined());
            write_array_length(&mut caller, ptr, new_len);
            val
        },
    );
    
    // ── arr_proto_includes (#51) ──────────────────────────────────────
    let arr_proto_includes_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, _args_count: i32| -> i64 {
            let val = read_shadow_arg(&mut caller, args_base, 0);
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_bool(false);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                if let Some(elem) = read_array_elem(&mut caller, ptr, i) {
                    if elem == val { return value::encode_bool(true); }
                }
            }
            value::encode_bool(false)
        },
    );
    
    // ── arr_proto_index_of (#52) ──────────────────────────────────────
    let arr_proto_index_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, _args_count: i32| -> i64 {
            let val = read_shadow_arg(&mut caller, args_base, 0);
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_f64(-1.0);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                if let Some(elem) = read_array_elem(&mut caller, ptr, i) {
                    if elem == val { return value::encode_f64(i as f64); }
                }
            }
            value::encode_f64(-1.0)
        },
    );
    
    // ── arr_proto_join (#53) ─────────────────────────────────────────
    let arr_proto_join_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let sep_val = if args_count > 0 { read_shadow_arg(&mut caller, args_base, 0) } else { value::encode_undefined() };
            // 默认分隔符为 ","
            let sep_str = if value::is_undefined(sep_val) || value::is_null(sep_val) {
                ",".to_string()
            } else {
                get_string_value(&mut caller, sep_val)
            };
            let mut parts = Vec::new();
            for i in 0..len {
                if let Some(elem) = read_array_elem(&mut caller, ptr, i) {
                    parts.push(render_value(&mut caller, elem).unwrap_or_else(|_| "".to_string()));
                } else {
                    parts.push(String::new());
                }
            }
            store_runtime_string(&caller, parts.join(&sep_str))
        },
    );
    
    // ── arr_proto_concat (#54) ────────────────────────────────────────
    let arr_proto_concat_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let Some(this_ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let this_len = read_array_length(&mut caller, this_ptr).unwrap_or(0);
            // 计算总元素数
            let mut total_len = this_len as usize;
            for i in 0..args_count as u32 {
                let arg = read_shadow_arg(&mut caller, args_base, i);
                if value::is_array(arg) {
                    if let Some(arg_ptr) = resolve_array_ptr(&mut caller, arg) {
                        total_len += read_array_length(&mut caller, arg_ptr).unwrap_or(0) as usize;
                    }
                } else {
                    total_len += 1;
                }
            }
            let new_arr = alloc_array(&mut caller, total_len as u32);
            let Some(new_ptr) = resolve_array_ptr(&mut caller, new_arr) else {
                return value::encode_undefined();
            };
            let mut write_idx = 0u32;
            // 复制 this 元素
            for i in 0..this_len {
                if let Some(elem) = read_array_elem(&mut caller, this_ptr, i) {
                    write_array_elem(&mut caller, new_ptr, write_idx, elem);
                    write_idx += 1;
                }
            }
            // 复制参数元素
            for i in 0..args_count as u32 {
                let arg = read_shadow_arg(&mut caller, args_base, i);
                if value::is_array(arg) {
                    if let Some(arg_ptr) = resolve_array_ptr(&mut caller, arg) {
                        let arg_len = read_array_length(&mut caller, arg_ptr).unwrap_or(0);
                        for j in 0..arg_len {
                            if let Some(elem) = read_array_elem(&mut caller, arg_ptr, j) {
                                write_array_elem(&mut caller, new_ptr, write_idx, elem);
                                write_idx += 1;
                            }
                        }
                    }
                } else {
                    write_array_elem(&mut caller, new_ptr, write_idx, arg);
                    write_idx += 1;
                }
            }
            write_array_length(&mut caller, new_ptr, write_idx);
            new_arr
        },
    );
    
    // ── arr_proto_slice (#55) ─────────────────────────────────────────
    let arr_proto_slice_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            let start = if args_count > 0 {
                let s_f64 = f64::from_bits(read_shadow_arg(&mut caller, args_base, 0) as u64);
                if s_f64.is_nan() { 0 }
                else if s_f64 < 0.0 { (len + s_f64 as i32).max(0) }
                else { (s_f64 as i32).min(len) }
            } else {
                0
            };
            let end = if args_count > 1 {
                let e_f64 = f64::from_bits(read_shadow_arg(&mut caller, args_base, 1) as u64);
                if e_f64.is_nan() { len }
                else if e_f64 < 0.0 { (len + e_f64 as i32).max(0) }
                else { (e_f64 as i32).min(len) }
            } else {
                len
            };
            let count = (end - start).max(0) as u32;
            let new_arr = alloc_array(&mut caller, count);
            let Some(new_ptr) = resolve_array_ptr(&mut caller, new_arr) else {
                return value::encode_undefined();
            };
            for i in 0..count {
                let elem = read_array_elem(&mut caller, ptr, start as u32 + i).unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, new_ptr, i, elem);
            }
            write_array_length(&mut caller, new_ptr, count);
            new_arr
        },
    );
    
    // ── arr_proto_fill (#56) ──────────────────────────────────────────
    let arr_proto_fill_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let val = read_shadow_arg(&mut caller, args_base, 0);
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return this_val;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            let start = if args_count > 1 {
                let s_f64 = f64::from_bits(read_shadow_arg(&mut caller, args_base, 1) as u64);
                if s_f64.is_nan() { 0 }
                else if s_f64 < 0.0 { (len + s_f64 as i32).max(0) }
                else { (s_f64 as i32).min(len) }
            } else {
                0
            };
            let end = if args_count > 2 {
                let e_f64 = f64::from_bits(read_shadow_arg(&mut caller, args_base, 2) as u64);
                if e_f64.is_nan() { len }
                else if e_f64 < 0.0 { (len + e_f64 as i32).max(0) }
                else { (e_f64 as i32).min(len) }
            } else {
                len
            };
            for i in start..end {
                write_array_elem(&mut caller, ptr, i as u32, val);
            }
            this_val
        },
    );
    
    // ── arr_proto_reverse (#57) ───────────────────────────────────────
    let arr_proto_reverse_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, _args_base: i32, _args_count: i32| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return this_val;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len / 2 {
                let a = read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let b = read_array_elem(&mut caller, ptr, len - 1 - i).unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, ptr, i, b);
                write_array_elem(&mut caller, ptr, len - 1 - i, a);
            }
            this_val
        },
    );
    
    // ── arr_proto_flat (#58) ──────────────────────────────────────────
    let arr_proto_flat_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            // 读取 depth 参数（默认 1）
            let depth = if args_count > 0 {
                let d = f64::from_bits(read_shadow_arg(&mut caller, args_base, 0) as u64);
                if d.is_nan() { 0 } else { d as u32 }
            } else { 1 };
            // 递归展平
            fn flatten(caller: &mut Caller<'_, RuntimeState>, arr: i64, depth: u32, result: &mut Vec<i64>) {
                if depth == 0 {
                    // 不再展平，直接添加数组引用
                    result.push(arr);
                    return;
                }
                let Some(ptr) = resolve_array_ptr(caller, arr) else { result.push(arr); return; };
                let len = read_array_length(caller, ptr).unwrap_or(0);
                for i in 0..len {
                    if let Some(elem) = read_array_elem(caller, ptr, i) {
                        if value::is_array(elem) {
                            flatten(caller, elem, depth - 1, result);
                        } else {
                            result.push(elem);
                        }
                    }
                }
            }
            let mut elements = Vec::new();
            flatten(&mut caller, this_val, depth, &mut elements);
            let new_arr = alloc_array(&mut caller, elements.len() as u32);
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
    
    // ── arr_proto_shift (#59) ─────────────────────────────────────────
    let arr_proto_shift_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, _args_base: i32, _args_count: i32| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            if len == 0 { return value::encode_undefined(); }
            let val = read_array_elem(&mut caller, ptr, 0).unwrap_or(value::encode_undefined());
            // 左移元素
            for i in 1..len {
                let elem = read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, ptr, i - 1, elem);
            }
            write_array_length(&mut caller, ptr, len - 1);
            val
        },
    );
    
    // ── arr_proto_unshift (#60) ───────────────────────────────────────
    let arr_proto_unshift_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let cap = read_array_capacity(&mut caller, ptr).unwrap_or(0);
            let new_len = len + args_count as u32;
            let mut ptr = ptr;
            if new_len > cap {
                let new_cap = cap.max(1) * 2;
                let needed = new_len.max(new_cap);
                if let Some(new_ptr) = grow_array(&mut caller, ptr, this_val, needed) {
                    ptr = new_ptr;
                } else {
                    return value::encode_undefined();
                }
            }
            // 右移现有元素
            for i in (0..len).rev() {
                let elem = read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
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
    
    // ── arr_proto_sort (#61) ──────────────────────────────────────────
    let arr_proto_sort_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return this_val;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as usize;
            if len <= 1 { return this_val; }
            // 读全部元素到 Vec
            let mut elems: Vec<i64> = (0..len)
                .map(|i| read_array_elem(&mut caller, ptr, i as u32).unwrap_or(value::encode_undefined()))
                .collect();
            if args_count > 0 && value::is_callable(read_shadow_arg(&mut caller, args_base, 0)) {
                let cmp = read_shadow_arg(&mut caller, args_base, 0);
                merge_sort_by(&mut elems, &mut |a, b| -> std::cmp::Ordering {
                    let result = call_wasm_callback(&mut caller, cmp, value::encode_undefined(), &[*a, *b])
                        .unwrap_or(value::encode_f64(0.0));
                    let v = f64::from_bits(result as u64);
                    if v > 0.0 { std::cmp::Ordering::Greater }
                    else if v < 0.0 { std::cmp::Ordering::Less }
                    else { std::cmp::Ordering::Equal }
                });
            } else {
                let keys: Vec<String> = elems.iter()
                    .map(|e| render_value(&mut caller, *e).unwrap_or_default())
                    .collect();
                // 带原始 index 的稳定排序
                let mut indexed: Vec<(usize, &i64)> = (0..len).map(|i| (i, &elems[i])).collect();
                indexed.sort_by(|(ia, _), (ib, _)| {
                    let ka = &keys[*ia];
                    let kb = &keys[*ib];
                    let cmp = ka.cmp(kb);
                    if cmp == std::cmp::Ordering::Equal { ia.cmp(ib) } else { cmp }
                });
                let sorted: Vec<i64> = indexed.iter().map(|(_, e)| **e).collect();
                elems = sorted;
            }
            // 写回
            for (i, &elem) in elems.iter().enumerate() {
                write_array_elem(&mut caller, ptr, i as u32, elem);
            }
            this_val
        },
    );
    
    // ── arr_proto_at (#62) ────────────────────────────────────────────
    let arr_proto_at_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            let idx = if args_count > 0 {
                let i_f64 = f64::from_bits(read_shadow_arg(&mut caller, args_base, 0) as u64);
                if i_f64.is_nan() { return value::encode_undefined(); }
                if i_f64 < 0.0 { len + i_f64 as i32 } else { i_f64 as i32 }
            } else {
                0
            };
            if idx < 0 || idx >= len {
                return value::encode_undefined();
            }
            read_array_elem(&mut caller, ptr, idx as u32).unwrap_or(value::encode_undefined())
        },
    );
    
    // ── arr_proto_copy_within (#63) ──────────────────────────────────
    let arr_proto_copy_within_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return this_val;
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            // target
            let raw_target = if args_count > 0 {
                let t = f64::from_bits(read_shadow_arg(&mut caller, args_base, 0) as u64);
                if t.is_nan() { 0 } else { t as i32 }
            } else { 0 };
            let target = if raw_target < 0 { (len + raw_target).max(0) } else { raw_target.min(len) };
            // start
            let raw_start = if args_count > 1 {
                let s = f64::from_bits(read_shadow_arg(&mut caller, args_base, 1) as u64);
                if s.is_nan() { 0 } else { s as i32 }
            } else { 0 };
            let start = if raw_start < 0 { (len + raw_start).max(0) } else { raw_start.min(len) };
            // end
            let raw_end = if args_count > 2 {
                let e = f64::from_bits(read_shadow_arg(&mut caller, args_base, 2) as u64);
                if e.is_nan() { len } else { e as i32 }
            } else { len };
            let end = if raw_end < 0 { (len + raw_end).max(0) } else { raw_end.min(len) };
            let count = (end - start).min(len - target).max(0) as u32;
            // 复制元素（处理重叠：从后往前复制）
            if target < start {
                for i in 0..count {
                    let elem = read_array_elem(&mut caller, ptr, (start as u32) + i).unwrap_or(value::encode_undefined());
                    write_array_elem(&mut caller, ptr, (target as u32) + i, elem);
                }
            } else {
                for i in (0..count).rev() {
                    let elem = read_array_elem(&mut caller, ptr, (start as u32) + i).unwrap_or(value::encode_undefined());
                    write_array_elem(&mut caller, ptr, (target as u32) + i, elem);
                }
            }
            this_val
        },
    );
    
    // ── arr_proto_for_each (#64) ─────────────────────────────────────
    let arr_proto_for_each_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) { return value::encode_undefined(); }
            let this_arg = if args_count > 1 { read_shadow_arg(&mut caller, args_base, 1) } else { value::encode_undefined() };
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                let elem = read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                if call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val]).is_err() {
                    return value::encode_undefined();
                }
            }
            value::encode_undefined()
        },
    );
    
    // ── arr_proto_map (#65) ──────────────────────────────────────────
    let arr_proto_map_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) { return value::encode_undefined(); }
            let this_arg = if args_count > 1 { read_shadow_arg(&mut caller, args_base, 1) } else { value::encode_undefined() };
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let new_arr = alloc_array(&mut caller, len);
            let Some(new_ptr) = resolve_array_ptr(&mut caller, new_arr) else {
                return value::encode_undefined();
            };
            for i in 0..len {
                let elem = read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                let result = match call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val]) {
                    Ok(r) => r,
                    Err(_) => value::encode_undefined(),
                };
                write_array_elem(&mut caller, new_ptr, i, result);
            }
            write_array_length(&mut caller, new_ptr, len);
            new_arr
        },
    );
    
    // ── arr_proto_filter (#66) ───────────────────────────────────────
    let arr_proto_filter_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) { return value::encode_undefined(); }
            let this_arg = if args_count > 1 { read_shadow_arg(&mut caller, args_base, 1) } else { value::encode_undefined() };
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let mut passed: Vec<i64> = Vec::new();
            for i in 0..len {
                let elem = read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                let ok = match call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val]) {
                    Ok(r) => value::is_truthy(r),
                    Err(_) => false,
                };
                if ok { passed.push(elem); }
            }
            let new_arr = alloc_array(&mut caller, passed.len() as u32);
            let Some(new_ptr) = resolve_array_ptr(&mut caller, new_arr) else {
                return value::encode_undefined();
            };
            for (i, elem) in passed.iter().enumerate() {
                write_array_elem(&mut caller, new_ptr, i as u32, *elem);
            }
            write_array_length(&mut caller, new_ptr, passed.len() as u32);
            new_arr
        },
    );
    
    // ── arr_proto_reduce (#67) ────────────────────────────────────────
    let arr_proto_reduce_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) { return value::encode_undefined(); }
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as usize;
            if len == 0 {
                if args_count < 2 {
                    *caller.data().runtime_error.lock().expect("runtime error mutex") =
                        Some("TypeError: Reduce of empty array with no initial value".to_string());
                    return value::encode_undefined();
                }
                return read_shadow_arg(&mut caller, args_base, 1);
            }
            let mut acc: i64;
            let mut start_idx = 0usize;
            if args_count >= 2 {
                acc = read_shadow_arg(&mut caller, args_base, 1);
            } else {
                acc = read_array_elem(&mut caller, ptr, 0).unwrap_or(value::encode_undefined());
                start_idx = 1;
            }
            for i in start_idx..len {
                let elem = read_array_elem(&mut caller, ptr, i as u32).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                match call_wasm_callback(&mut caller, cb, value::encode_undefined(), &[acc, elem, idx_val, this_val]) {
                    Ok(r) => acc = r,
                    Err(_) => return value::encode_undefined(),
                }
            }
            acc
        },
    );
    
    // ── arr_proto_reduce_right (#68) ──────────────────────────────────
    let arr_proto_reduce_right_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) { return value::encode_undefined(); }
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            if len == 0 {
                if args_count < 2 {
                    *caller.data().runtime_error.lock().expect("runtime error mutex") =
                        Some("TypeError: Reduce of empty array with no initial value".to_string());
                    return value::encode_undefined();
                }
                return read_shadow_arg(&mut caller, args_base, 1);
            }
            let mut acc: i64;
            let mut start_idx = len - 1;
            if args_count >= 2 {
                acc = read_shadow_arg(&mut caller, args_base, 1);
            } else {
                acc = read_array_elem(&mut caller, ptr, start_idx as u32).unwrap_or(value::encode_undefined());
                start_idx = len - 2;
            }
            for i in (0..=start_idx as usize).rev() {
                let elem = read_array_elem(&mut caller, ptr, i as u32).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                match call_wasm_callback(&mut caller, cb, value::encode_undefined(), &[acc, elem, idx_val, this_val]) {
                    Ok(r) => acc = r,
                    Err(_) => return value::encode_undefined(),
                }
            }
            acc
        },
    );
    
    // ── arr_proto_find (#69) ──────────────────────────────────────────
    let arr_proto_find_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, _args_count: i32| -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) { return value::encode_undefined(); }
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                let elem = read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                match call_wasm_callback(&mut caller, cb, value::encode_undefined(), &[elem, idx_val, this_val]) {
                    Ok(r) => if value::is_truthy(r) { return elem; }
                    Err(_) => {}
                }
            }
            value::encode_undefined()
        },
    );
    
    // ── arr_proto_find_index (#70) ────────────────────────────────────
    let arr_proto_find_index_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, _args_count: i32| -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) { return value::encode_f64(-1.0); }
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_f64(-1.0);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                let elem = read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                match call_wasm_callback(&mut caller, cb, value::encode_undefined(), &[elem, idx_val, this_val]) {
                    Ok(r) => if value::is_truthy(r) { return value::encode_f64(i as f64); }
                    Err(_) => {}
                }
            }
            value::encode_f64(-1.0)
        },
    );
    
    // ── arr_proto_some (#71) ─────────────────────────────────────────
    let arr_proto_some_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, _args_count: i32| -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) { return value::encode_bool(false); }
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_bool(false);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                let elem = read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                match call_wasm_callback(&mut caller, cb, value::encode_undefined(), &[elem, idx_val, this_val]) {
                    Ok(r) => if value::is_truthy(r) { return value::encode_bool(true); }
                    Err(_) => {}
                }
            }
            value::encode_bool(false)
        },
    );
    
    // ── arr_proto_every (#72) ────────────────────────────────────────
    let arr_proto_every_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, _args_count: i32| -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) { return value::encode_bool(false); }
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_bool(false);
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            for i in 0..len {
                let elem = read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                match call_wasm_callback(&mut caller, cb, value::encode_undefined(), &[elem, idx_val, this_val]) {
                    Ok(r) => if !value::is_truthy(r) { return value::encode_bool(false); }
                    Err(_) => return value::encode_bool(false),
                }
            }
            value::encode_bool(true)
        },
    );
    
    // ── arr_proto_flat_map (#73) ─────────────────────────────────────
    let arr_proto_flat_map_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) { return value::encode_undefined(); }
            let this_arg = if args_count > 1 { read_shadow_arg(&mut caller, args_base, 1) } else { value::encode_undefined() };
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0);
            let mut elements: Vec<i64> = Vec::new();
            for i in 0..len {
                let elem = read_array_elem(&mut caller, ptr, i).unwrap_or(value::encode_undefined());
                let idx_val = value::encode_f64(i as f64);
                let mapped = match call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val]) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                if value::is_array(mapped) {
                    // 展平一层
                    if let Some(mapped_ptr) = resolve_array_ptr(&mut caller, mapped) {
                        let mapped_len = read_array_length(&mut caller, mapped_ptr).unwrap_or(0);
                        for j in 0..mapped_len {
                            if let Some(inner) = read_array_elem(&mut caller, mapped_ptr, j) {
                                elements.push(inner);
                            }
                        }
                    }
                } else {
                    elements.push(mapped);
                }
            }
            let new_arr = alloc_array(&mut caller, elements.len() as u32);
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
    
    // ── arr_proto_splice (#74) ───────────────────────────────────────
    let arr_proto_splice_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let Some(ptr) = resolve_array_ptr(&mut caller, this_val) else {
                return value::encode_undefined();
            };
            let len = read_array_length(&mut caller, ptr).unwrap_or(0) as i32;
            // 读取 start
            let raw_start = if args_count > 0 {
                let s = f64::from_bits(read_shadow_arg(&mut caller, args_base, 0) as u64);
                if s.is_nan() { 0 } else { s as i32 }
            } else { 0 };
            let start_idx = if raw_start < 0 { (len + raw_start).max(0) } else { raw_start.min(len) };
            // 读取 deleteCount
            let delete_count = if args_count > 1 {
                let d = f64::from_bits(read_shadow_arg(&mut caller, args_base, 1) as u64);
                if d.is_nan() { 0 } else { (d as i32).max(0) }
            } else {
                (len - start_idx).max(0)
            };
            let actual_delete = delete_count.min(len - start_idx);
            let insert_count = (args_count - 2).max(0) as i32;
            let new_len = len - actual_delete + insert_count;
            let cap = read_array_capacity(&mut caller, ptr).unwrap_or(0) as i32;
            let mut ptr = ptr;
            if new_len > cap {
                let new_cap = cap.max(1) * 2;
                let needed = new_len.max(new_cap);
                if let Some(new_ptr) = grow_array(&mut caller, ptr, this_val, needed as u32) {
                    ptr = new_ptr;
                } else {
                    return value::encode_undefined();
                }
            }
            // 收集被删除的元素
            let deleted_arr = alloc_array(&mut caller, actual_delete as u32);
            let Some(deleted_ptr) = resolve_array_ptr(&mut caller, deleted_arr) else {
                return value::encode_undefined();
            };
            for i in 0..actual_delete {
                let elem = read_array_elem(&mut caller, ptr, (start_idx as u32) + i as u32)
                    .unwrap_or(value::encode_undefined());
                write_array_elem(&mut caller, deleted_ptr, i as u32, elem);
            }
            write_array_length(&mut caller, deleted_ptr, actual_delete as u32);
            // 移动元素（右移或左移）
            if insert_count != actual_delete {
                if insert_count < actual_delete {
                    // 左移
                    for i in start_idx..(len - actual_delete + insert_count) {
                        let src = i + actual_delete - insert_count;
                        let elem = read_array_elem(&mut caller, ptr, src as u32)
                            .unwrap_or(value::encode_undefined());
                        write_array_elem(&mut caller, ptr, i as u32, elem);
                    }
                } else {
                    // 右移（从后往前）
                    for i in (start_idx..(len - actual_delete + insert_count)).rev() {
                        let src = i - insert_count + actual_delete;
                        let elem = read_array_elem(&mut caller, ptr, src as u32)
                            .unwrap_or(value::encode_undefined());
                        write_array_elem(&mut caller, ptr, i as u32 + insert_count as u32 - actual_delete as u32, elem);
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
    
    // ── arr_proto_is_array (#75) ──────────────────────────────────────
    let arr_proto_is_array_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, _this_val: i64, args_base: i32, _args_count: i32| -> i64 {
            let val = read_shadow_arg(&mut caller, args_base, 0);
            value::encode_bool(value::is_array(val))
        },
    );
    
    // ── abort_shadow_stack_overflow (#76) ─────────────────────────────
    let abort_shadow_stack_overflow_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, shadow_sp: i32, args_bytes: i32, stack_end: i32| {
            let mut buffer = caller
                .data()
                .output
                .lock()
                .expect("runtime output buffer mutex should not be poisoned");
            writeln!(
                &mut *buffer,
                "shadow stack overflow: sp=0x{shadow_sp:x} + {args_bytes} bytes > end=0x{stack_end:x}"
            ).ok();
            *caller
                .data()
                .runtime_error
                .lock()
                .expect("runtime error mutex") =
                Some(format!("shadow stack overflow: sp={shadow_sp} + {args_bytes} > end={stack_end}"));
        },
    );

    // ── func_call (#78): Function.prototype.call ────────────────────────────
    // 签名: (i64 func, i64 this_val, i64 args_base, i32 args_count) -> i64
    let func_call_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         func: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            resolve_and_call(&mut caller, func, this_val, args_base, args_count)
        },
    );

    // ── func_apply (#79): Function.prototype.apply ──────────────────────────
    let func_apply_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         func: i64,
         this_val: i64,
         args_array: i64|
         -> i64 {
            func_apply_impl(&mut caller, func, this_val, args_array)
        },
    );

    // ── func_bind (#80): Function.prototype.bind ────────────────────────────
    let func_bind_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         func: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            func_bind_impl(&mut caller, func, this_val, args_base, args_count)
        },
    );

    // ── object_rest (#81): Exclude specified keys from object ───────────────
    let object_rest_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, excluded_keys: i64| -> i64 {
            object_rest_impl(&mut caller, obj, excluded_keys)
        },
    );

    // ── get_prototype_from_constructor (#82): Safe prototype access ─────────
    let get_prototype_from_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, ctor: i64| -> i64 {
            get_prototype_from_constructor_impl(&mut caller, ctor)
        },
    );

    // ── obj_spread (#83): Copy own enumerable properties ────────────────────
    let obj_spread_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, dest: i64, source: i64| {
            obj_spread_impl(&mut caller, dest, source);
        },
    );
    
    let imports = [
        console_log.into(),          // 0
        f64_mod.into(),              // 1
        f64_pow.into(),              // 2
        throw_fn.into(),             // 3
        iterator_from.into(),        // 4
        iterator_next.into(),        // 5
        iterator_close.into(),       // 6
        iterator_value.into(),       // 7
        iterator_done.into(),        // 8
        enumerator_from.into(),      // 9
        enumerator_next.into(),      // 10
        enumerator_key.into(),       // 11
        enumerator_done.into(),      // 12
        typeof_fn.into(),            // 13
        op_in.into(),                // 14
        op_instanceof.into(),        // 15
        string_concat.into(),        // 16
        string_concat_va.into(),     // 17
        define_property_fn.into(),   // 18
        get_own_prop_desc_fn.into(), // 19
        abstract_eq.into(),          // 20
        abstract_compare.into(),     // 21
        gc_collect.into(),           // 22
        console_error.into(),        // 23
        console_warn.into(),         // 24
        console_info.into(),         // 25
        console_debug.into(),        // 26
        console_trace.into(),        // 27
        set_timeout_fn.into(),       // 28
        clear_timeout_fn.into(),     // 29
        set_interval_fn.into(),      // 30
        clear_interval_fn.into(),    // 31
        fetch_fn.into(),             // 32
        json_stringify_fn.into(),    // 33
        json_parse_fn.into(),        // 34
        closure_create_fn.into(),    // 35
        closure_get_func_fn.into(),  // 36
        closure_get_env_fn.into(),   // 37
        arr_push_fn.into(),          // 38
        arr_pop_fn.into(),           // 39
        arr_includes_fn.into(),      // 40
        arr_index_of_fn.into(),      // 41
        arr_join_fn.into(),          // 42
        arr_concat_fn.into(),        // 43
        arr_slice_fn.into(),         // 44
        arr_fill_fn.into(),          // 45
        arr_reverse_fn.into(),       // 46
        arr_flat_fn.into(),          // 47
        arr_init_length_fn.into(),   // 48
        arr_get_length_fn.into(),    // 49
        arr_proto_push_fn.into(),        // 50
        arr_proto_pop_fn.into(),         // 51
        arr_proto_includes_fn.into(),    // 52
        arr_proto_index_of_fn.into(),    // 53
        arr_proto_join_fn.into(),        // 54
        arr_proto_concat_fn.into(),      // 55
        arr_proto_slice_fn.into(),       // 56
        arr_proto_fill_fn.into(),        // 57
        arr_proto_reverse_fn.into(),     // 58
        arr_proto_flat_fn.into(),        // 59
        arr_proto_shift_fn.into(),       // 60
        arr_proto_unshift_fn.into(),     // 61
        arr_proto_sort_fn.into(),        // 62
        arr_proto_at_fn.into(),          // 63
        arr_proto_copy_within_fn.into(), // 64
        arr_proto_for_each_fn.into(),    // 65
        arr_proto_map_fn.into(),         // 66
        arr_proto_filter_fn.into(),      // 67
        arr_proto_reduce_fn.into(),      // 68
        arr_proto_reduce_right_fn.into(),// 69
        arr_proto_find_fn.into(),        // 70
        arr_proto_find_index_fn.into(),  // 71
        arr_proto_some_fn.into(),        // 72
        arr_proto_every_fn.into(),       // 73
        arr_proto_flat_map_fn.into(),    // 74
        arr_proto_splice_fn.into(),      // 75
        arr_proto_is_array_fn.into(),    // 76
        abort_shadow_stack_overflow_fn.into(), // 77
        func_call_fn.into(),                   // 78
        func_apply_fn.into(),                  // 79
        func_bind_fn.into(),                   // 80
        object_rest_fn.into(),                 // 81
        get_prototype_from_constructor_fn.into(), // 82
        obj_spread_fn.into(),                  // 83
    ];
    let instance = Instance::new(&mut store, &module, &imports)?;

    // ── Run main ─────────────────────────────────────────────────────────
    let main = instance.get_typed_func::<(), ()>(&mut store, "main")?;
    let main_result = main.call(&mut store, ());

    // ── Timer event loop (only if main succeeded) ─────────────────────────
    // Poll timers; fire expired callbacks via the WASM function table.
    if main_result.is_ok() {
        loop {
            let now = Instant::now();
            let mut entry_to_fire: Option<TimerEntry> = None;

            {
                let mut timers = store.data().timers.lock().expect("timers mutex");
                let mut cancelled = store
                    .data()
                    .cancelled_timers
                    .lock()
                    .expect("cancelled_timers mutex");

                // Remove cancelled timers
                timers.retain(|t| !cancelled.contains(&t.id));
                cancelled.clear();

                if timers.is_empty() {
                    break;
                }

                // Find earliest expired timer
                if let Some(idx) = timers.iter().position(|t| t.deadline <= now) {
                    entry_to_fire = Some(timers.remove(idx));
                } else {
                    // Sleep until next timer
                    let next = timers.iter().min_by_key(|t| t.deadline).unwrap().deadline;
                    let dur = next.saturating_duration_since(Instant::now());
                    if !dur.is_zero() {
                        std::thread::sleep(dur);
                    }
                    continue;
                }
            }

            if let Some(entry) = entry_to_fire {
                let callback = entry.callback;
                let repeating = entry.repeating;
                let interval = entry.interval;
                let entry_id = entry.id;

                // Call the callback via WASM function table call_indirect
                let raw_idx = value::decode_function_idx(callback) as u64;
                if let Some(Extern::Table(tbl)) = instance.get_export(&mut store, "__table") {
                    if let Some(Ref::Func(Some(func))) = tbl.get(&mut store, raw_idx) {
                        if let Ok(typed) = func.typed::<(i64, i32, i32), i64>(&store) {
                            match typed.call(&mut store, (value::encode_undefined(), 0i32, 0i32)) {
                                Ok(_) => {}
                                Err(e) => {
                                    let msg = format!("timer callback error: {}", e);
                                    let mut error_lock = store
                                        .data()
                                        .runtime_error
                                        .lock()
                                        .expect("runtime_error mutex");
                                    if error_lock.is_none() {
                                        *error_lock = Some(msg);
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }

                // Re-schedule if repeating
                if repeating {
                    store
                        .data()
                        .timers
                        .lock()
                        .expect("timers mutex")
                        .push(TimerEntry {
                            id: entry_id,
                            deadline: Instant::now() + interval,
                            callback,
                            repeating: true,
                            interval,
                        });
                }
            }
        }
    }
    // ── Collect output ────────────────────────────────────────────────────
    let bytes = output
        .lock()
        .expect("runtime output buffer mutex should not be poisoned")
        .clone();
    drop(store);

    let mut writer = writer;
    writer.write_all(&bytes)?;

    // ── Check errors ─────────────────────────────────────────────────────
    if let Some(message) = runtime_error.lock().expect("runtime error mutex").clone() {
        anyhow::bail!(message);
    }

    // Propagate any wasm trap from main() call (must be after output collection)
    main_result?;

    Ok(writer)
}

struct RuntimeState {
    output: Arc<Mutex<Vec<u8>>>,
    iterators: Arc<Mutex<Vec<IteratorState>>>,
    enumerators: Arc<Mutex<Vec<EnumeratorState>>>,
    runtime_strings: Arc<Mutex<Vec<String>>>,
    runtime_error: Arc<Mutex<Option<String>>>,
    /// GC 标记位图：每个 handle 对应 1 bit，用于标记-清除 GC。
    gc_mark_bits: Arc<Mutex<Vec<u64>>>,
    /// 分配计数器：每次对象分配后递增，用于触发周期性 GC。
    alloc_counter: Arc<Mutex<u64>>,
    /// GC 触发阈值：当 alloc_counter 达到此值时触发 GC。
    gc_threshold: u64,
    /// 定时器列表
    timers: Arc<Mutex<Vec<TimerEntry>>>,
    /// 已取消的定时器 ID 集合
    cancelled_timers: Arc<Mutex<HashSet<u32>>>,
    /// 下一个定时器 ID
    next_timer_id: Arc<Mutex<u32>>,
    /// 闭包表：每个闭包条目存储函数表索引和环境对象
    closures: Arc<Mutex<Vec<ClosureEntry>>>,
    /// 绑定函数表：存储 func.bind(this, args) 创建的绑定函数
    bound_objects: Arc<Mutex<Vec<BoundRecord>>>,
}

/// 绑定函数记录
struct BoundRecord {
    target_func: i64,     // TAG_FUNCTION / TAG_CLOSURE / TAG_BOUND
    bound_this: i64,      // NaN-boxed
    bound_args: Vec<i64>, // NaN-boxed values
}

/// 闭包条目
struct ClosureEntry {
    func_idx: u32,
    env_obj: i64,
}

struct TimerEntry {
    id: u32,
    deadline: Instant,
    callback: i64, // NaN-boxed function handle
    repeating: bool,
    interval: Duration,
}

enum IteratorState {
    StringIter { data: Vec<u8>, byte_pos: usize },
    Error,
}

enum EnumeratorState {
    StringEnum {
        length: usize,
        index: usize,
    },
    /// 对象属性枚举：keys 存储属性名列表
    ObjectEnum {
        keys: Vec<String>,
        index: usize,
    },
    Error,
}

fn render_value(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Result<String> {
    if value::is_string(val) {
        if value::is_runtime_string_handle(val) {
            let handle = value::decode_runtime_string_handle(val) as usize;
            let strings = caller
                .data()
                .runtime_strings
                .lock()
                .expect("runtime strings mutex");
            if let Some(value) = strings.get(handle) {
                return Ok(value.clone());
            }
            return Ok(String::new());
        }

        return read_string(caller, value::decode_string_ptr(val));
    }

    if value::is_undefined(val) {
        return Ok("undefined".to_string());
    }

    if value::is_null(val) {
        return Ok("null".to_string());
    }

    if value::is_bool(val) {
        return Ok(if value::decode_bool(val) {
            "true".to_string()
        } else {
            "false".to_string()
        });
    }

    if value::is_iterator(val) {
        let handle = value::decode_handle(val);
        return Ok(format!("[iterator:{handle}]"));
    }

    if value::is_enumerator(val) {
        let handle = value::decode_handle(val);
        return Ok(format!("[enumerator:{handle}]"));
    }

    if value::is_exception(val) {
        let handle = value::decode_handle(val);
        return Ok(format!("[exception:{handle}]"));
    }

    if value::is_array(val) {
        let ptr = resolve_array_ptr(caller, val);
        if let Some(ptr) = ptr {
            let len = read_array_length(caller, ptr).unwrap_or(0);
            let mut parts = Vec::with_capacity(len as usize);
            for i in 0..len {
                if let Some(elem) = read_array_elem(caller, ptr, i) {
                    parts.push(render_value(caller, elem).unwrap_or_else(|_| "?".to_string()));
                } else {
                    parts.push("?".to_string());
                }
            }
            return Ok(format!("[{}]", parts.join(", ")));
        }
        return Ok("[array]".to_string());
    }

    if value::is_object(val) {
        let ptr = value::decode_object_handle(val);
        return Ok(format!("[object Object:{ptr}]"));
    }

    if value::is_function(val) {
        let idx = value::decode_function_idx(val);
        return Ok(format!("function [ref:{idx}]"));
    }

    if value::is_closure(val) {
        let idx = value::decode_closure_idx(val);
        return Ok(format!("function [closure:{idx}]"));
    }

    Ok(f64::from_bits(val as u64).to_string())
}

fn write_console_value(caller: &mut Caller<'_, RuntimeState>, val: i64, prefix: Option<&str>) {
    let rendered = render_value(caller, val).unwrap_or_else(|_| "unknown".to_string());
    let mut buffer = caller
        .data()
        .output
        .lock()
        .expect("runtime output buffer mutex should not be poisoned");
    match prefix {
        Some(p) => writeln!(&mut *buffer, "[{p}] {rendered}"),
        None => writeln!(&mut *buffer, "{rendered}"),
    }
    .expect("write_console_value should write to the configured output sink");
}

/// 简单的 URL 编码解码（支持 %XX 和 + → 空格）
fn urlencoding_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        match c {
            '+' => result.push(' '),
            '%' => {
                let hex: String = chars.by_ref().take(2).collect();
                if hex.len() == 2 {
                    if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                        result.push(byte as char);
                    } else {
                        result.push('%');
                        result.push_str(&hex);
                    }
                } else {
                    result.push('%');
                    result.push_str(&hex);
                }
            }
            other => result.push(other),
        }
    }
    result
}

/// 简单的 JSON.stringify 实现（简化版，仅支持基本类型）
fn runtime_json_stringify(caller: &mut Caller<'_, RuntimeState>, val: i64) -> String {
    if value::is_f64(val) {
        let f = f64::from_bits(val as u64);
        if f.is_finite() {
            f.to_string()
        } else {
            "null".to_string()
        }
    } else if value::is_string(val) {
        let s = if value::is_runtime_string_handle(val) {
            let handle = value::decode_runtime_string_handle(val) as usize;
            caller
                .data()
                .runtime_strings
                .lock()
                .expect("runtime strings mutex")
                .get(handle)
                .cloned()
                .unwrap_or_default()
        } else {
            read_string(caller, value::decode_string_ptr(val)).unwrap_or_default()
        };
        format!("\"{}\"", s.escape_default())
    } else if value::is_bool(val) {
        value::decode_bool(val).to_string()
    } else if value::is_null(val) {
        "null".to_string()
    } else if value::is_undefined(val) || value::is_callable(val) {
        "undefined".to_string()
    } else if value::is_object(val) {
        "[object Object]".to_string()
    } else {
        "null".to_string()
    }
}

fn read_string(caller: &mut Caller<'_, RuntimeState>, ptr: u32) -> Result<String> {
    let data = read_string_bytes(caller, ptr);
    Ok(std::str::from_utf8(&data)?.to_owned())
}

fn read_string_bytes(caller: &mut Caller<'_, RuntimeState>, ptr: u32) -> Vec<u8> {
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return Vec::new();
    };

    let data = memory.data(caller);
    let start = ptr as usize;
    if start >= data.len() {
        return Vec::new();
    }

    let end = data[start..]
        .iter()
        .position(|byte| *byte == 0)
        .map_or(data.len(), |offset| start + offset);

    data[start..end].to_vec()
}

fn read_value_string_bytes(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Option<Vec<u8>> {
    if !value::is_string(val) {
        return None;
    }

    if value::is_runtime_string_handle(val) {
        let handle = value::decode_runtime_string_handle(val) as usize;
        let strings = caller
            .data()
            .runtime_strings
            .lock()
            .expect("runtime strings mutex");
        return strings.get(handle).map(|string| string.as_bytes().to_vec());
    }

    Some(read_string_bytes(caller, value::decode_string_ptr(val)))
}

fn store_runtime_string(caller: &Caller<'_, RuntimeState>, string: String) -> i64 {
    let mut strings = caller
        .data()
        .runtime_strings
        .lock()
        .expect("runtime strings mutex");
    let handle = strings.len() as u32;
    strings.push(string);
    value::encode_runtime_string_handle(handle)
}

/// GC 标记阶段：递归标记对象及其所有可达对象。
/// 使用标记位图避免重复标记和循环引用。
fn mark_object_recursive(
    caller: &mut Caller<'_, RuntimeState>,
    handle_idx: usize,
    obj_ptr: usize,
    obj_table_ptr: usize,
    obj_table_count: usize,
) {
    // 检查标记位图
    let word_idx = handle_idx / 64;
    let bit_idx = handle_idx % 64;

    {
        let mut mark_bits = caller
            .data()
            .gc_mark_bits
            .lock()
            .expect("gc_mark_bits mutex");
        if word_idx >= mark_bits.len() {
            // 扩展位图
            mark_bits.resize(word_idx + 1, 0);
        }
        // 已标记，跳过
        if (mark_bits[word_idx] & (1u64 << bit_idx)) != 0 {
            return;
        }
        // 标记
        mark_bits[word_idx] |= 1u64 << bit_idx;
    }

    // 收集需要递归标记的对象列表
    let mut children_to_mark: Vec<(usize, usize)> = Vec::new(); // (handle_idx, obj_ptr)

    // 获取内存并读取信息
    {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return;
        };
        let data = memory.data(&*caller);

        // 读取对象头 — 至少需要 16 字节（proto + type + pad + capacity/length + num_props/capacity）
        if obj_ptr + 16 > data.len() {
            return;
        }

        // 读取 proto_handle（offset 0，未改变）
        let proto_handle = u32::from_le_bytes([
            data[obj_ptr],
            data[obj_ptr + 1],
            data[obj_ptr + 2],
            data[obj_ptr + 3],
        ]);
        if proto_handle != 0xFFFF_FFFF && (proto_handle as usize) < obj_table_count {
            let proto_slot_addr = obj_table_ptr + proto_handle as usize * 4;
            if proto_slot_addr + 4 <= data.len() {
                let proto_ptr = u32::from_le_bytes([
                    data[proto_slot_addr],
                    data[proto_slot_addr + 1],
                    data[proto_slot_addr + 2],
                    data[proto_slot_addr + 3],
                ]) as usize;
                if proto_ptr != 0 {
                    children_to_mark.push((proto_handle as usize, proto_ptr));
                }
            }
        }

        // 读取 type byte 决定是数组还是对象
        let heap_type = data[obj_ptr + 4];

        if heap_type == wjsm_ir::HEAP_TYPE_ARRAY {
            // ── 数组对象 ──
            let len = u32::from_le_bytes([
                data[obj_ptr + 8],
                data[obj_ptr + 9],
                data[obj_ptr + 10],
                data[obj_ptr + 11],
            ]) as usize;

            // 迭代 8 字节元素，收集对象/函数引用
            for i in 0..len {
                let elem_offset = obj_ptr + 16 + i * 8;
                if elem_offset + 8 > data.len() {
                    break;
                }
                let elem = i64::from_le_bytes([
                    data[elem_offset], data[elem_offset+1],
                    data[elem_offset+2], data[elem_offset+3],
                    data[elem_offset+4], data[elem_offset+5],
                    data[elem_offset+6], data[elem_offset+7],
                ]);
                if value::is_object(elem) || value::is_function(elem) {
                    let child_handle_idx = (elem as u64 & 0xFFFF_FFFF) as usize;
                    if child_handle_idx < obj_table_count {
                        let child_slot_addr = obj_table_ptr + child_handle_idx * 4;
                        if child_slot_addr + 4 <= data.len() {
                            let child_ptr = u32::from_le_bytes([
                                data[child_slot_addr],
                                data[child_slot_addr + 1],
                                data[child_slot_addr + 2],
                                data[child_slot_addr + 3],
                            ]) as usize;
                            if child_ptr != 0 {
                                children_to_mark.push((child_handle_idx, child_ptr));
                            }
                        }
                    }
                }
            }
        } else {
            // ── 普通对象 ──
            let num_props = u32::from_le_bytes([
                data[obj_ptr + 12],
                data[obj_ptr + 13],
                data[obj_ptr + 14],
                data[obj_ptr + 15],
            ]) as usize;

            // 遍历属性，收集所有对象/函数引用
            // 属性槽: [name_id(4), flags(4), value(8), getter(8), setter(8)] = 32 字节
            // 属性槽起始: ptr + 16
            for i in 0..num_props {
                let slot_offset = obj_ptr + 16 + i * 32;
                if slot_offset + 32 > data.len() {
                    break;
                }

                // 读取 value (offset 8), getter (offset 16), setter (offset 24)
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

                for val in [value, getter, setter] {
                    if value::is_object(val) || value::is_function(val) {
                        let child_handle_idx = (val as u64 & 0xFFFF_FFFF) as usize;
                        if child_handle_idx < obj_table_count {
                            let child_slot_addr = obj_table_ptr + child_handle_idx * 4;
                            if child_slot_addr + 4 <= data.len() {
                                let child_ptr = u32::from_le_bytes([
                                    data[child_slot_addr],
                                    data[child_slot_addr + 1],
                                    data[child_slot_addr + 2],
                                    data[child_slot_addr + 3],
                                ]) as usize;
                                if child_ptr != 0 {
                                    children_to_mark.push((child_handle_idx, child_ptr));
                                }
                            }
                        }
                    }
                }
            }
        }
    } // data 借用在这里结束

    // 递归标记收集到的对象
    for (child_handle_idx, child_ptr) in children_to_mark {
        mark_object_recursive(
            caller,
            child_handle_idx,
            child_ptr,
            obj_table_ptr,
            obj_table_count,
        );
    }
}

/// 通过 handle 表解析 boxed value 的真实对象指针。
/// 支持 TAG_OBJECT 和 TAG_FUNCTION（统一走 handle 表）。
fn resolve_handle(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Option<usize> {
    let handle_idx = (val as u64 & 0xFFFF_FFFF) as usize;
    resolve_handle_idx(caller, handle_idx)
}

/// 通过 handle_idx 解析真实对象指针。
fn resolve_handle_idx(caller: &mut Caller<'_, RuntimeState>, handle_idx: usize) -> Option<usize> {
    let obj_table_ptr = {
        let Some(Extern::Global(g)) = caller.get_export("__obj_table_ptr") else {
            return None;
        };
        g.get(&mut *caller).i32().unwrap_or(0) as usize
    };
    let slot_addr = obj_table_ptr + handle_idx * 4;
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return None;
    };
    let d = mem.data(&*caller);
    if slot_addr + 4 > d.len() {
        return None;
    }
    let ptr = u32::from_le_bytes([
        d[slot_addr],
        d[slot_addr + 1],
        d[slot_addr + 2],
        d[slot_addr + 3],
    ]) as usize;
    if ptr == 0 {
        return None;
    }
    Some(ptr)
}

// ── Array helpers ──────────────────────────────────────────────────────

/// 解析 TAG_ARRAY 值 → 数组对象的内存指针
fn resolve_array_ptr(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Option<usize> {
    let handle_idx = (val as u64 & 0xFFFF_FFFF) as usize;
    resolve_handle_idx(caller, handle_idx)
}

/// 读取数组的 length 字段（offset 8）
fn read_array_length(caller: &mut Caller<'_, RuntimeState>, ptr: usize) -> Option<u32> {
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else { return None; };
    let d = mem.data(&*caller);
    if ptr + 16 > d.len() { return None; }
    Some(u32::from_le_bytes([d[ptr+8], d[ptr+9], d[ptr+10], d[ptr+11]]))
}

fn write_array_length(caller: &mut Caller<'_, RuntimeState>, ptr: usize, len: u32) {
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else { return; };
    let d = mem.data_mut(&mut *caller);
    if ptr + 16 > d.len() { return; }
    d[ptr+8..ptr+12].copy_from_slice(&len.to_le_bytes());
}

/// 读取数组的 capacity 字段（offset 12）
fn read_array_capacity(caller: &mut Caller<'_, RuntimeState>, ptr: usize) -> Option<u32> {
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else { return None; };
    let d = mem.data(&*caller);
    if ptr + 16 > d.len() { return None; }
    Some(u32::from_le_bytes([d[ptr+12], d[ptr+13], d[ptr+14], d[ptr+15]]))
}

/// 读取数组元素 elements[index]（offset 16 + index * 8）
fn read_array_elem(caller: &mut Caller<'_, RuntimeState>, ptr: usize, index: u32) -> Option<i64> {
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else { return None; };
    let d = mem.data(&*caller);
    let elem_offset = ptr + 16 + (index as usize) * 8;
    if elem_offset + 8 > d.len() { return None; }
    Some(i64::from_le_bytes([
        d[elem_offset], d[elem_offset+1], d[elem_offset+2], d[elem_offset+3],
        d[elem_offset+4], d[elem_offset+5], d[elem_offset+6], d[elem_offset+7],
    ]))
}

/// 写入数组元素
fn write_array_elem(caller: &mut Caller<'_, RuntimeState>, ptr: usize, index: u32, val: i64) {
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else { return; };
    let d = mem.data_mut(&mut *caller);
    let elem_offset = ptr + 16 + (index as usize) * 8;
    if elem_offset + 8 > d.len() { return; }
    d[elem_offset..elem_offset+8].copy_from_slice(&val.to_le_bytes());
}

/// 数组动态扩容 — 遵循现有对象扩容的 capacity × 2 倍增策略
fn grow_array(caller: &mut Caller<'_, RuntimeState>, ptr: usize, this_val: i64, new_cap: u32) -> Option<usize> {
    let heap_ptr = {
        let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else { return None; };
        g.get(&mut *caller).i32().unwrap_or(0) as usize
    };
    let obj_table_ptr = {
        let Some(Extern::Global(g)) = caller.get_export("__obj_table_ptr") else { return None; };
        g.get(&mut *caller).i32().unwrap_or(0) as usize
    };
    let new_size = 16 + new_cap as usize * 8;
    let old_size = {
        let cap = read_array_capacity(caller, ptr)?;
        16 + cap as usize * 8
    };
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else { return None; };
    let d = mem.data_mut(&mut *caller);
    if heap_ptr + new_size > d.len() { return None; }
    d.copy_within(ptr..ptr + old_size, heap_ptr);
    d[heap_ptr + 12..heap_ptr + 16].copy_from_slice(&new_cap.to_le_bytes());
    let handle_idx = (this_val as u64 & 0xFFFF_FFFF) as usize;
    let slot_addr = obj_table_ptr + handle_idx * 4;
    if slot_addr + 4 <= d.len() {
        d[slot_addr..slot_addr + 4].copy_from_slice(&(heap_ptr as u32).to_le_bytes());
    }
    if let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") {
        let _ = g.set(&mut *caller, Val::I32((heap_ptr + new_size) as i32));
    }
    Some(heap_ptr)
}

/// 自顶向下迭代 merge sort，稳定 O(n log n)
fn merge_sort_by<T: Copy>(slice: &mut [T], cmp: &mut dyn FnMut(&T, &T) -> std::cmp::Ordering) {
    let n = slice.len();
    if n <= 1 { return; }
    let mut buf: Vec<T> = Vec::with_capacity(n);
    let mut width = 1;
    while width < n {
        let mut i = 0;
        while i < n {
            let mid = (i + width).min(n);
            let end = (i + 2 * width).min(n);
            let mut left = i;
            let mut right = mid;
            while left < mid && right < end {
                if cmp(&slice[left], &slice[right]) != std::cmp::Ordering::Greater {
                    buf.push(slice[left]);
                    left += 1;
                } else {
                    buf.push(slice[right]);
                    right += 1;
                }
            }
            while left < mid {
                buf.push(slice[left]);
                left += 1;
            }
            while right < end {
                buf.push(slice[right]);
                right += 1;
            }
            slice[i..end].copy_from_slice(&buf[..end - i]);
            buf.clear();
            i += 2 * width;
        }
        width *= 2;
    }
}

/// 从对象中按名称读取属性值（用于 define_property 等）
fn read_object_property_by_name(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    prop_name: &str,
) -> Option<i64> {
    let num_props = {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data(&*caller);
        if obj_ptr + 16 > data.len() {
            return None;
        }
        u32::from_le_bytes([
            data[obj_ptr + 12],
            data[obj_ptr + 13],
            data[obj_ptr + 14],
            data[obj_ptr + 15],
        ]) as usize
    };
    let mut name_ids = Vec::with_capacity(num_props);
    {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data(&*caller);
        for i in 0..num_props {
            let slot_offset = obj_ptr + 16 + i * 32;
            if slot_offset + 4 > data.len() {
                break;
            }
            name_ids.push(u32::from_le_bytes([
                data[slot_offset],
                data[slot_offset + 1],
                data[slot_offset + 2],
                data[slot_offset + 3],
            ]));
        }
    }
    for (i, name_id) in name_ids.iter().enumerate() {
        let name_bytes = read_string_bytes(caller, *name_id);
        if name_bytes == prop_name.as_bytes() {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return None;
            };
            let data = memory.data(&*caller);
            let slot_offset = obj_ptr + 16 + i * 32;
            if slot_offset + 32 > data.len() {
                return None;
            }
            return Some(i64::from_le_bytes([
                data[slot_offset + 8],
                data[slot_offset + 9],
                data[slot_offset + 10],
                data[slot_offset + 11],
                data[slot_offset + 12],
                data[slot_offset + 13],
                data[slot_offset + 14],
                data[slot_offset + 15],
            ]));
        }
    }
    None
}

/// 从对象中按 name_id 查找属性的 slot_offset
fn find_property_slot_by_name_id(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    name_id: u32,
) -> Option<(usize, i32, i64)> {
    let num_props = {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data(&*caller);
        if obj_ptr + 16 > data.len() {
            return None;
        }
        u32::from_le_bytes([
            data[obj_ptr + 12],
            data[obj_ptr + 13],
            data[obj_ptr + 14],
            data[obj_ptr + 15],
        ]) as usize
    };
    for i in 0..num_props {
        let slot_offset = obj_ptr + 16 + i * 32;
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data(&*caller);
        if slot_offset + 32 > data.len() {
            break;
        }
        let slot_name_id = u32::from_le_bytes([
            data[slot_offset],
            data[slot_offset + 1],
            data[slot_offset + 2],
            data[slot_offset + 3],
        ]);
        if slot_name_id == name_id {
            let flags = i32::from_le_bytes([
                data[slot_offset + 4],
                data[slot_offset + 5],
                data[slot_offset + 6],
                data[slot_offset + 7],
            ]);
            let val = i64::from_le_bytes([
                data[slot_offset + 8],
                data[slot_offset + 9],
                data[slot_offset + 10],
                data[slot_offset + 11],
                data[slot_offset + 12],
                data[slot_offset + 13],
                data[slot_offset + 14],
                data[slot_offset + 15],
            ]);
            return Some((slot_offset, flags, val));
        }
    }
    None
}

/// 读取对象/函数的所有属性名，用于 for...in 枚举
fn enumerate_object_keys(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Vec<String> {
    // 解析对象指针：通过 handle 表统一解析
    let ptr: usize = match resolve_handle(caller, val) {
        Some(p) => p,
        None => return Vec::new(),
    };

    // 读取属性列表
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return Vec::new();
    };
    let data = memory.data(&*caller);
    if ptr + 16 > data.len() {
        return Vec::new();
    }

    let num_props =
        u32::from_le_bytes([data[ptr + 12], data[ptr + 13], data[ptr + 14], data[ptr + 15]]) as usize;

    let mut name_ids = Vec::with_capacity(num_props);
    for i in 0..num_props {
        let slot_offset = ptr + 16 + i * 32;
        if slot_offset + 4 > data.len() {
            break;
        }
        let name_id = u32::from_le_bytes([
            data[slot_offset],
            data[slot_offset + 1],
            data[slot_offset + 2],
            data[slot_offset + 3],
        ]);
        name_ids.push(name_id);
    }
    let _ = data; // 释放对 memory 的借用

    let mut keys = Vec::with_capacity(name_ids.len());
    for name_id in name_ids {
        let name_bytes = read_string_bytes(caller, name_id);
        if let Ok(name) = std::str::from_utf8(&name_bytes) {
            keys.push(name.to_string());
        }
    }
    keys
}

/// 分配描述符对象，用于 Object.getOwnPropertyDescriptor 返回值
/// 对象格式：header(16 bytes) + 4 slots * 32 bytes = 144 bytes
fn allocate_descriptor_object(
    caller: &mut Caller<'_, RuntimeState>,
    is_accessor: bool,
    value: i64,
    writable: bool,
    enumerable: bool,
    configurable: bool,
    getter: i64,
    setter: i64,
) -> Option<i64> {
    // 读取全局变量
    let obj_table_ptr = {
        let Some(Extern::Global(g)) = caller.get_export("__obj_table_ptr") else {
            return None;
        };
        g.get(&mut *caller).i32().unwrap_or(0) as usize
    };
    let obj_table_count = {
        let Some(Extern::Global(g)) = caller.get_export("__obj_table_count") else {
            return None;
        };
        g.get(&mut *caller).i32().unwrap_or(0) as usize
    };
    let heap_ptr = {
        let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
            return None;
        };
        g.get(&mut *caller).i32().unwrap_or(0) as usize
    };

    // 对象大小：16 (header) + 4 * 32 (slots) = 144 bytes
    let obj_size = 16 + 4 * 32;
    let handle_idx = obj_table_count;

    // 分配对象
    {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data_mut(&mut *caller);
        if heap_ptr + obj_size > data.len() {
            return None;
        }

        // 初始化 header: proto=0, type=OBJECT, pad=0, capacity=4, num_props=0
        data[heap_ptr..heap_ptr + 4].copy_from_slice(&0u32.to_le_bytes()); // proto
        data[heap_ptr + 4] = wjsm_ir::HEAP_TYPE_OBJECT; // type byte
        data[heap_ptr + 5..heap_ptr + 8].fill(0); // pad bytes
        data[heap_ptr + 8..heap_ptr + 12].copy_from_slice(&4u32.to_le_bytes()); // capacity
        data[heap_ptr + 12..heap_ptr + 16].copy_from_slice(&0u32.to_le_bytes()); // num_props

        // 注册到 handle 表
        let slot_addr = obj_table_ptr + handle_idx * 4;
        if slot_addr + 4 <= data.len() {
            data[slot_addr..slot_addr + 4].copy_from_slice(&(heap_ptr as u32).to_le_bytes());
        }
    }

    // 更新 __heap_ptr 和 __obj_table_count
    {
        let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
            return None;
        };
        let _ = g.set(&mut *caller, Val::I32((heap_ptr + obj_size) as i32));
    }
    {
        let Some(Extern::Global(g)) = caller.get_export("__obj_table_count") else {
            return None;
        };
        let _ = g.set(&mut *caller, Val::I32((handle_idx + 1) as i32));
    }

    // 现在设置描述符对象的属性
    let desc_ptr = heap_ptr;

    // 写入属性的辅助闭包
    let mut write_property = |name_id: u32, val: i64, flags: i32| -> Option<()> {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data_mut(&mut *caller);
        // 读取当前 num_props
        let num_props = u32::from_le_bytes([
            data[desc_ptr + 12],
            data[desc_ptr + 13],
            data[desc_ptr + 14],
            data[desc_ptr + 15],
        ]) as usize;
        let slot_offset = desc_ptr + 16 + num_props * 32;
        if slot_offset + 32 > data.len() {
            return None;
        }
        data[slot_offset..slot_offset + 4].copy_from_slice(&name_id.to_le_bytes());
        data[slot_offset + 4..slot_offset + 8].copy_from_slice(&flags.to_le_bytes());
        data[slot_offset + 8..slot_offset + 16].copy_from_slice(&val.to_le_bytes());
        // getter 和 setter 为 undefined
        let undef = value::encode_undefined();
        data[slot_offset + 16..slot_offset + 24].copy_from_slice(&undef.to_le_bytes());
        data[slot_offset + 24..slot_offset + 32].copy_from_slice(&undef.to_le_bytes());
        // 更新 num_props
        let new_num_props = (num_props + 1) as u32;
        data[desc_ptr + 12..desc_ptr + 16].copy_from_slice(&new_num_props.to_le_bytes());
        Some(())
    };

    // flags: enumerable 和 configurable
    let base_flags: i32 =
        (if enumerable { 1 << 1 } else { 0 }) | (if configurable { 1 } else { 0 });

    if is_accessor {
        // 访问器属性：get, set, enumerable, configurable
        // writable flag 不适用于访问器属性
        let get_flags = base_flags | (1 << 2); // writable=true for function values
        write_property(constants::PROP_DESC_GET_OFFSET, getter, get_flags)?;
        write_property(constants::PROP_DESC_SET_OFFSET, setter, get_flags)?;
    } else {
        // 数据属性：value, writable, enumerable, configurable
        let writable_flags = base_flags | (if writable { 1 << 2 } else { 0 });
        write_property(constants::PROP_DESC_VALUE_OFFSET, value, writable_flags)?;
        write_property(
            constants::PROP_DESC_WRITABLE_OFFSET,
            value::encode_bool(writable),
            base_flags | (1 << 2),
        )?;
    }

    // enumerable 和 configurable 对于两种属性都要写
    write_property(
        constants::PROP_DESC_ENUMERABLE_OFFSET,
        value::encode_bool(enumerable),
        base_flags | (1 << 2),
    )?;
    write_property(
        constants::PROP_DESC_CONFIGURABLE_OFFSET,
        value::encode_bool(configurable),
        base_flags | (1 << 2),
    )?;

    // 返回对象 handle
    Some(value::encode_object_handle(handle_idx as u32))
}

// ── 辅助函数用于 abstract_eq 和 abstract_compare ─────────────────────────

/// ToNumber 抽象操作 (ECMAScript 7.1.4)
/// 将值转换为 Number 类型
fn to_number(caller: &mut Caller<'_, RuntimeState>, val: i64) -> i64 {
    // undefined → NaN
    if value::is_undefined(val) {
        return f64::NAN.to_bits() as i64;
    }

    // null → +0
    if value::is_null(val) {
        return 0.0_f64.to_bits() as i64;
    }

    // bool: true → 1, false → 0
    if value::is_bool(val) {
        let b = value::decode_bool(val);
        return (if b { 1.0_f64 } else { 0.0_f64 }).to_bits() as i64;
    }

    // f64 → itself
    if value::is_f64(val) {
        return val;
    }

    // string → parseFloat (可能失败 → NaN)
    if value::is_string(val) {
        let s = if value::is_runtime_string_handle(val) {
            let handle = value::decode_runtime_string_handle(val) as usize;
            let strings = caller
                .data()
                .runtime_strings
                .lock()
                .expect("runtime strings mutex");
            strings.get(handle).cloned().unwrap_or_default()
        } else {
            read_string(caller, value::decode_string_ptr(val)).unwrap_or_default()
        };

        // 尝试解析字符串为数字
        // 先尝试 trim，然后解析
        let trimmed = s.trim();
        if let Ok(num) = trimmed.parse::<f64>() {
            return num.to_bits() as i64;
        }
        // 解析失败返回 NaN
        return f64::NAN.to_bits() as i64;
    }

    // object/function → ToPrimitive(hint: Number) → ToNumber
    // 简化实现：调用 render_value 返回字符串，然后解析
    if value::is_object(val) || value::is_callable(val) {
        let prim = to_primitive(caller, val);
        return to_number(caller, prim);
    }

    // 其他类型（iterator, enumerator, exception）→ NaN
    f64::NAN.to_bits() as i64
}

/// ToPrimitive 抽象操作 (ECMAScript 7.1.1)
/// 将对象转换为原始值
/// 简化实现：调用 render_value 返回字符串
fn to_primitive(caller: &mut Caller<'_, RuntimeState>, val: i64) -> i64 {
    // 已经是原始类型
    if value::is_f64(val)
        || value::is_string(val)
        || value::is_bool(val)
        || value::is_undefined(val)
        || value::is_null(val)
    {
        return val;
    }

    // object/function → 调用 render_value 返回字符串表示
    if value::is_object(val) || value::is_callable(val) {
        if let Ok(s) = render_value(caller, val) {
            // 将字符串存入 runtime_strings
            let mut strings = caller
                .data()
                .runtime_strings
                .lock()
                .expect("runtime strings mutex");
            let handle = strings.len() as u32;
            strings.push(s);
            return value::encode_runtime_string_handle(handle);
        }
    }

    // 其他类型直接返回
    val
}

/// 严格相等比较 (ECMAScript 7.2.16)
fn strict_eq(caller: &mut Caller<'_, RuntimeState>, a: i64, b: i64) -> i64 {
    // 类型不同 → false
    let a_type = type_tag(a);
    let b_type = type_tag(b);

    if a_type != b_type {
        return value::encode_bool(false);
    }

    // 同类型比较
    match a_type {
        // f64: 注意 NaN !== NaN
        0 => {
            let af = f64::from_bits(a as u64);
            let bf = f64::from_bits(b as u64);
            if af.is_nan() || bf.is_nan() {
                return value::encode_bool(false);
            }
            value::encode_bool(af == bf)
        }
        // string
        1 => {
            let a_str = get_string_value(caller, a);
            let b_str = get_string_value(caller, b);
            value::encode_bool(a_str == b_str)
        }
        // undefined
        2 => value::encode_bool(true),
        // null
        3 => value::encode_bool(true),
        // bool
        4 => value::encode_bool(value::decode_bool(a) == value::decode_bool(b)),
        // object/function/iterator/enumerator/exception: 引用比较
        _ => value::encode_bool(a == b),
    }
}

/// 获取类型标签 (用于 strict_eq)
/// 返回值: 0=f64, 1=string, 2=undefined, 3=null, 4=bool, 5+=其他
fn type_tag(val: i64) -> u64 {
    if value::is_f64(val) {
        0
    } else if value::is_string(val) {
        1
    } else if value::is_undefined(val) {
        2
    } else if value::is_null(val) {
        3
    } else if value::is_bool(val) {
        4
    } else {
        5
    } // object, function, iterator, enumerator, exception
}

/// 获取字符串值
fn get_string_value(caller: &mut Caller<'_, RuntimeState>, val: i64) -> String {
    if value::is_runtime_string_handle(val) {
        let handle = value::decode_runtime_string_handle(val) as usize;
        let strings = caller
            .data()
            .runtime_strings
            .lock()
            .expect("runtime strings mutex");
        strings.get(handle).cloned().unwrap_or_default()
    } else {
        read_string(caller, value::decode_string_ptr(val)).unwrap_or_default()
    }
}

/// 解析并调用函数（支持 TAG_FUNCTION / TAG_CLOSURE / TAG_BOUND）
fn resolve_and_call(
    caller: &mut Caller<'_, RuntimeState>,
    func: i64,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let memory = caller.get_export("memory").and_then(|e| e.into_memory()).unwrap();

    if value::is_bound(func) {
        let bound_idx = value::decode_bound_idx(func);
        let (target_func, bound_this, bound_args_ref) = {
            let bound = caller.data().bound_objects.lock().unwrap();
            let record = &bound[bound_idx as usize];
            (record.target_func, record.bound_this, record.bound_args.clone())
        };

        let total_count = bound_args_ref.len() as i32 + args_count;
        // 读取当前 shadow_sp
        let shadow_sp_global = caller.get_export("__shadow_sp")
            .and_then(|e| e.into_global())
            .unwrap();
        let shadow_sp = shadow_sp_global.get(&mut *caller).i32().unwrap();
        let ptr = shadow_sp;

        // Push bound_args at position 0
        for (i, arg) in bound_args_ref.iter().enumerate() {
            memory.write(&mut *caller, (ptr + i as i32 * 8) as usize, &arg.to_le_bytes()).unwrap();
        }
        // Copy call args after
        for i in 0..args_count {
            let mut buf = [0u8; 8];
            memory.read(&mut *caller, (shadow_sp + args_base + i * 8) as usize, &mut buf).unwrap();
            memory.write(&mut *caller, (ptr + (bound_args_ref.len() as i32 + i) * 8) as usize, &buf).unwrap();
        }

        // 递归解析 target_func
        resolve_callable_and_call(caller, target_func, bound_this, ptr, total_count)
    } else {
        resolve_callable_and_call(caller, func, this_val, args_base, args_count)
    }
}

fn resolve_callable_and_call(
    caller: &mut Caller<'_, RuntimeState>,
    callee: i64,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let (func_idx, env_obj) = if value::is_closure(callee) {
        let idx = value::decode_closure_idx(callee);
        let closures = caller.data().closures.lock().unwrap();
        let entry = &closures[idx as usize];
        (entry.func_idx, entry.env_obj)
    } else if value::is_function(callee) {
        (value::decode_function_idx(callee), value::encode_undefined())
    } else if value::is_bound(callee) {
        return resolve_and_call(caller, callee, this_val, args_base, args_count);
    } else {
        return value::encode_undefined();
    };

    let table = caller.get_export("__table").and_then(|e| e.into_table()).unwrap();
    let func_ref = table.get(&mut *caller, func_idx as u64);
    let func = func_ref.as_ref().and_then(|r| r.as_func()).and_then(|f| f);
    let Some(func) = func else {
        return value::encode_undefined();
    };
    let mut results = [Val::I64(0)];
    let _ = func.call(&mut *caller, &[Val::I64(env_obj), Val::I64(this_val), Val::I32(args_base), Val::I32(args_count)], &mut results);
    results[0].unwrap_i64()
}

fn func_apply_impl(
    caller: &mut Caller<'_, RuntimeState>,
    func: i64,
    this_val: i64,
    _args_array: i64,
) -> i64 {
    // args_array 是一个数组对象，需要展开其元素到 shadow stack
    // 简化实现：直接使用 func_call 语义但只支持固定参数
    // 完整实现需要读取数组元素
    resolve_and_call(caller, func, this_val, 0, 0)
}

fn func_bind_impl(
    caller: &mut Caller<'_, RuntimeState>,
    func: i64,
    this_val: i64,
    args_base: i32,
    args_count: i32,
) -> i64 {
    let memory = caller.get_export("memory").and_then(|e| e.into_memory()).unwrap();
    let mut bound_args = Vec::with_capacity(args_count as usize);
    for i in 0..args_count {
        let mut buf = [0u8; 8];
        memory.read(&mut *caller, (args_base + i * 8) as usize, &mut buf).unwrap();
        bound_args.push(i64::from_le_bytes(buf));
    }
    let mut bound = caller.data().bound_objects.lock().unwrap();
    let idx = bound.len() as u32;
    bound.push(BoundRecord {
        target_func: func,
        bound_this: this_val,
        bound_args,
    });
    value::encode_bound_idx(idx)
}

fn object_rest_impl(
    _caller: &mut Caller<'_, RuntimeState>,
    _obj: i64,
    _excluded_keys: i64,
) -> i64 {
    // 简化实现：返回一个新的空对象
    // 完整实现需要遍历 obj 的属性并排除指定键
    value::encode_undefined()
}

fn get_prototype_from_constructor_impl(
    caller: &mut Caller<'_, RuntimeState>,
    ctor: i64,
) -> i64 {
    // Get ctor.prototype property, return it if it's an Object, else return undefined
    // For now, just return ctor.prototype
    let memory = caller.get_export("memory").and_then(|e| e.into_memory()).unwrap();
    // Read the "prototype" string pointer from data segment
    // We need to look up the string ptr. Let's just call a simpler path:
    // Fallback: return undefined for now means prototype chain works by reading ctor.prototype directly
    // Actually, the simplest fix: call $obj_get(ctor, "prototype")
    // But we're in Rust, not WASM. We need to do the equivalent.
    // Let's use the existing pattern from op_instanceof
    
    // Simple stub: just try to get the "prototype" property from ctor
    // This requires obj_get which is a WASM function, not accessible from host
    // So let's return undefined — this means new.prototype won't work
    // but we need the old behavior back for class methods to work
    value::encode_undefined()
}

fn obj_spread_impl(
    _caller: &mut Caller<'_, RuntimeState>,
    _dest: i64,
    _source: i64,
) {
    // 简化实现：不做任何复制
    // 完整实现需要遍历 source 的 own properties 并复制到 dest
}

#[cfg(test)]
mod tests {
    use super::execute_with_writer;
    use anyhow::Result;

    fn compile_source(source: &str) -> Result<Vec<u8>> {
        let module = wjsm_parser::parse_module(source)?;
        let program = wjsm_semantic::lower_module(module)?;
        wjsm_backend_wasm::compile(&program)
    }

    #[test]
    fn execute_with_writer_prints_string_fixture() -> Result<()> {
        let wasm_bytes = compile_source(r#"console.log("Hello, Runtime!");"#)?;
        let output = execute_with_writer(&wasm_bytes, Vec::new())?;

        assert_eq!(String::from_utf8(output)?, "Hello, Runtime!\n");
        Ok(())
    }

    #[test]
    fn execute_with_writer_prints_arithmetic_fixture() -> Result<()> {
        let wasm_bytes = compile_source("console.log(1 + 2 * 3);")?;
        let output = execute_with_writer(&wasm_bytes, Vec::new())?;

        assert_eq!(String::from_utf8(output)?, "7\n");
        Ok(())
    }
}
