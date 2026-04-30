use anyhow::Result;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use wasmtime::*;
use wjsm_ir::value;

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
    let mut store = Store::new(
        &engine,
        RuntimeState {
            output: Arc::clone(&output),
            iterators: Arc::clone(&iterators),
            enumerators: Arc::clone(&enumerators),
            runtime_strings: Arc::clone(&runtime_strings),
            runtime_error: Arc::clone(&runtime_error),
        },
    );

    // ── Import 0: console_log(i64) → () ─────────────────────────────────
    let console_log = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            let rendered =
                render_value(&mut caller, val).expect("console_log should render runtime values");
            let mut buffer = caller
                .data()
                .output
                .lock()
                .expect("runtime output buffer mutex should not be poisoned");
            writeln!(&mut *buffer, "{rendered}")
                .expect("console_log should write to the configured output sink");
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
                enums.push(EnumeratorState::StringEnum { length: 0, index: 0 });
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
            } else if value::is_function(val) {
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
                *caller.data().runtime_error.lock().expect("runtime error mutex") =
                    Some("TypeError: cannot use 'in' operator on non-object".to_string());
                return value::encode_bool(false);
            }

            // 获取属性名（ToPropertyKey 转换）
            let prop_str = if value::is_string(prop) {
                if value::is_runtime_string_handle(prop) {
                    let handle = value::decode_runtime_string_handle(prop) as usize;
                    let strings = caller.data().runtime_strings.lock().expect("runtime strings mutex");
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

            // 解析对象指针：object 直接解码，function 需要查 func_props
            // 注意：函数路径需要可变借用 caller，因此分开处理
            let mut ptr = if value::is_object(object) {
                value::decode_object_handle(object) as usize
            } else {
                // 函数: 通过 func_props_ptr 全局变量查找属性对象
                let func_props_ptr = {
                    let Some(Extern::Global(g)) = caller.get_export("__func_props_ptr") else {
                        return value::encode_bool(false);
                    };
                    g.get(&mut caller).i32().unwrap_or(0) as usize
                };
                let func_idx = value::decode_function_idx(object) as usize;
                let slot_addr = func_props_ptr + func_idx * 8;
                let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                    return value::encode_bool(false);
                };
                let d = mem.data(&caller);
                if slot_addr + 4 > d.len() {
                    return value::encode_bool(false);
                }
                let obj_ptr = u32::from_le_bytes([
                    d[slot_addr], d[slot_addr + 1],
                    d[slot_addr + 2], d[slot_addr + 3],
                ]) as usize;
                if obj_ptr == 0 {
                    return value::encode_bool(false);
                }
                obj_ptr
            };

            // 搜索属性，遍历原型链
            loop {
                // 读取对象属性
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return value::encode_bool(false);
                };
                let data = memory.data(&caller);
                if ptr + 12 > data.len() {
                    return value::encode_bool(false);
                }

                let num_props = u32::from_le_bytes([
                    data[ptr + 8], data[ptr + 9],
                    data[ptr + 10], data[ptr + 11],
                ]) as usize;

                let name_ids: Vec<u32> = (0..num_props)
                    .filter_map(|i| {
                        let slot_offset = ptr + 12 + i * 12;
                        if slot_offset + 4 <= data.len() {
                            Some(u32::from_le_bytes([
                                data[slot_offset], data[slot_offset + 1],
                                data[slot_offset + 2], data[slot_offset + 3],
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
                let proto = u32::from_le_bytes([
                    data[ptr], data[ptr + 1],
                    data[ptr + 2], data[ptr + 3],
                ]);

                if proto == 0 {
                    return value::encode_bool(false);
                }
                ptr = proto as usize;
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
                *caller.data().runtime_error.lock().expect("runtime error mutex") =
                    Some("TypeError: Right-hand side of instanceof is not an object".to_string());
                return value::encode_undefined();
            }

            // 3. 获取 constructor 的属性列表并查找 "prototype" 属性
            let ctor_ptr = if value::is_object(constructor) {
                value::decode_object_handle(constructor) as usize
            } else {
                let func_props_ptr = {
                    let Some(Extern::Global(g)) = caller.get_export("__func_props_ptr") else {
                        return value::encode_bool(false);
                    };
                    g.get(&mut caller).i32().unwrap_or(0) as usize
                };
                let func_idx = value::decode_function_idx(constructor) as usize;
                let slot_addr = func_props_ptr + func_idx * 8;
                let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                    return value::encode_bool(false);
                };
                let d = mem.data(&caller);
                if slot_addr + 4 > d.len() {
                    return value::encode_bool(false);
                }
                let obj_ptr = u32::from_le_bytes([
                    d[slot_addr], d[slot_addr + 1],
                    d[slot_addr + 2], d[slot_addr + 3],
                ]) as usize;
                if obj_ptr == 0 {
                    return value::encode_bool(false);
                }
                obj_ptr
            };

            // 扫描 constructor 的属性查找 "prototype"
            let props: Vec<(u32, [u8; 8])> = {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return value::encode_bool(false);
                };
                let data = memory.data(&caller);
                if ctor_ptr + 12 > data.len() {
                    return value::encode_bool(false);
                }
                let num_props = u32::from_le_bytes([
                    data[ctor_ptr + 8], data[ctor_ptr + 9],
                    data[ctor_ptr + 10], data[ctor_ptr + 11],
                ]) as usize;

                (0..num_props)
                    .filter_map(|i| {
                        let slot_offset = ctor_ptr + 12 + i * 12;
                        if slot_offset + 12 <= data.len() {
                            let name_id = u32::from_le_bytes([
                                data[slot_offset], data[slot_offset + 1],
                                data[slot_offset + 2], data[slot_offset + 3],
                            ]);
                            let val_bytes = [
                                data[slot_offset + 4], data[slot_offset + 5],
                                data[slot_offset + 6], data[slot_offset + 7],
                                data[slot_offset + 8], data[slot_offset + 9],
                                data[slot_offset + 10], data[slot_offset + 11],
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
                    *caller.data().runtime_error.lock().expect("runtime error mutex") =
                        Some("TypeError: Function has non-object prototype property".to_string());
                    return value::encode_undefined();
                }
            };

            // 5. 遍历 value 的原型链
            let proto_target = value::decode_object_handle(prototype) as u32;
            let mut current = value;
            loop {
                let ptr = if value::is_object(current) {
                    value::decode_object_handle(current) as usize
                } else if value::is_function(current) {
                    let func_props_ptr = {
                        let Some(Extern::Global(g)) = caller.get_export("__func_props_ptr") else {
                            return value::encode_bool(false);
                        };
                        g.get(&mut caller).i32().unwrap_or(0) as usize
                    };
                    let func_idx = value::decode_function_idx(current) as usize;
                    let slot_addr = func_props_ptr + func_idx * 8;
                    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                        return value::encode_bool(false);
                    };
                    let d = mem.data(&caller);
                    if slot_addr + 4 > d.len() {
                        return value::encode_bool(false);
                    }
                    let obj_ptr = u32::from_le_bytes([
                        d[slot_addr], d[slot_addr + 1],
                        d[slot_addr + 2], d[slot_addr + 3],
                    ]) as usize;
                    if obj_ptr == 0 {
                        return value::encode_bool(false);
                    }
                    obj_ptr
                } else {
                    return value::encode_bool(false);
                };

                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return value::encode_bool(false);
                };
                let data = memory.data(&caller);
                if ptr + 4 > data.len() {
                    return value::encode_bool(false);
                }
                let proto = u32::from_le_bytes([
                    data[ptr], data[ptr + 1],
                    data[ptr + 2], data[ptr + 3],
                ]);

                if proto == 0 {
                    return value::encode_bool(false);
                }
                if proto == proto_target {
                    return value::encode_bool(true);
                }
                current = value::encode_object_handle(proto);
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
                    render_value(&mut caller, a).unwrap_or_default().into_bytes()
                };
                let b_s = if value::is_string(b) {
                    read_value_string_bytes(&mut caller, b).unwrap_or_default()
                } else {
                    render_value(&mut caller, b).unwrap_or_default().into_bytes()
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

    let imports = [
        console_log.into(),     // 0
        f64_mod.into(),         // 1
        f64_pow.into(),         // 2
        throw_fn.into(),        // 3
        iterator_from.into(),   // 4
        iterator_next.into(),   // 5
        iterator_close.into(),  // 6
        iterator_value.into(),  // 7
        iterator_done.into(),   // 8
        enumerator_from.into(), // 9
        enumerator_next.into(), // 10
        enumerator_key.into(),  // 11
        enumerator_done.into(), // 12
        typeof_fn.into(),       // 13
        op_in.into(),           // 14
        op_instanceof.into(),   // 15
        string_concat.into(),   // 16
    ];
    let instance = Instance::new(&mut store, &module, &imports)?;

    let main = instance.get_typed_func::<(), ()>(&mut store, "main")?;
    let call_result = main.call(&mut store, ());

    drop(store);

    let bytes = output
        .lock()
        .expect("runtime output buffer mutex should not be poisoned")
        .clone();
    let mut writer = writer;
    writer.write_all(&bytes)?;

    if let Some(message) = runtime_error.lock().expect("runtime error mutex").clone() {
        anyhow::bail!(message);
    }

    call_result?;
    Ok(writer)
}

struct RuntimeState {
    output: Arc<Mutex<Vec<u8>>>,
    iterators: Arc<Mutex<Vec<IteratorState>>>,
    enumerators: Arc<Mutex<Vec<EnumeratorState>>>,
    runtime_strings: Arc<Mutex<Vec<String>>>,
    runtime_error: Arc<Mutex<Option<String>>>,
}

enum IteratorState {
    StringIter { data: Vec<u8>, byte_pos: usize },
    Error,
}

enum EnumeratorState {
    StringEnum { length: usize, index: usize },
    /// 对象属性枚举：keys 存储属性名列表
    ObjectEnum { keys: Vec<String>, index: usize },
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

    if value::is_object(val) {
        let ptr = value::decode_object_handle(val);
        return Ok(format!("[object Object:{ptr}]"));
    }

    if value::is_function(val) {
        let idx = value::decode_function_idx(val);
        return Ok(format!("function [ref:{idx}]"));
    }

    Ok(f64::from_bits(val as u64).to_string())
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

/// 读取对象/函数的所有属性名，用于 for...in 枚举
fn enumerate_object_keys(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Vec<String> {
    // 解析对象指针
    let ptr: usize = if value::is_object(val) {
        value::decode_object_handle(val) as usize
    } else if value::is_function(val) {
        let func_props_ptr = {
            let Some(Extern::Global(g)) = caller.get_export("__func_props_ptr") else {
                return Vec::new();
            };
            g.get(&mut *caller).i32().unwrap_or(0) as usize
        };
        let func_idx = value::decode_function_idx(val) as usize;
        let slot_addr = func_props_ptr + func_idx * 8;
        let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
            return Vec::new();
        };
        let d = mem.data(&*caller);
        if slot_addr + 4 > d.len() {
            return Vec::new();
        }
        let obj_ptr = u32::from_le_bytes([
            d[slot_addr], d[slot_addr + 1],
            d[slot_addr + 2], d[slot_addr + 3],
        ]) as usize;
        if obj_ptr == 0 {
            return Vec::new();
        }
        obj_ptr
    } else {
        return Vec::new();
    };

    // 读取属性列表
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return Vec::new();
    };
    let data = memory.data(&*caller);
    if ptr + 12 > data.len() {
        return Vec::new();
    }

    let num_props = u32::from_le_bytes([
        data[ptr + 8], data[ptr + 9],
        data[ptr + 10], data[ptr + 11],
    ]) as usize;

    let mut name_ids = Vec::with_capacity(num_props);
    for i in 0..num_props {
        let slot_offset = ptr + 12 + i * 12;
        if slot_offset + 4 > data.len() {
            break;
        }
        let name_id = u32::from_le_bytes([
            data[slot_offset], data[slot_offset + 1],
            data[slot_offset + 2], data[slot_offset + 3],
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
