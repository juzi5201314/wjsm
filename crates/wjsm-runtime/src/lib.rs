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

    let mut store = Store::new(
        &engine,
        RuntimeState {
            output: Arc::clone(&output),
            iterators: Arc::clone(&iterators),
            enumerators: Arc::clone(&enumerators),
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
    let f64_mod = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
        let af = f64::from_bits(a as u64);
        let bf = f64::from_bits(b as u64);
        let result = af - bf * (af / bf).trunc();
        result.to_bits() as i64
    });

    // ── Import 2: f64_pow(i64, i64) → i64 ───────────────────────────────
    let f64_pow = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
        let af = f64::from_bits(a as u64);
        let bf = f64::from_bits(b as u64);
        let result = af.powf(bf);
        result.to_bits() as i64
    });

    // ── Import 3: throw(i64) → () ────────────────────────────────────────
    let throw_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, val: i64| {
        let rendered = render_value(&mut caller, val).unwrap_or_else(|_| "unknown".to_string());
        // Write error message to output
        let mut buffer = caller
            .data()
            .output
            .lock()
            .expect("runtime output buffer mutex should not be poisoned");
        writeln!(&mut *buffer, "Uncaught exception: {rendered}").ok();
        // Trap to halt execution
    });

    // ── Import 4: iterator_from(i64) → i64 ──────────────────────────────
    let iterator_from = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            if value::is_string(val) {
                let ptr = value::decode_string_ptr(val);
                // Read string from WASM memory
                let string_data = read_string_bytes(&mut caller, ptr);
                let mut iters = caller
                    .data()
                    .iterators
                    .lock()
                    .expect("iterators mutex");
                let handle = iters.len() as u32;
                iters.push(IteratorState::StringIter {
                    data: string_data,
                    byte_pos: 0,
                });
                value::encode_handle(value::TAG_ITERATOR, handle)
            } else {
                // Non-iterable: store an error state
                let mut iters = caller
                    .data()
                    .iterators
                    .lock()
                    .expect("iterators mutex");
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
            let mut iters = caller
                .data()
                .iterators
                .lock()
                .expect("iterators mutex");
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

    // ── Import 7: iterator_value(i64) → i64 ─────────────────────────────
    let iterator_value = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, handle: i64| -> i64 {
            let handle_idx = value::decode_handle(handle) as usize;
            let mut iters = caller
                .data()
                .iterators
                .lock()
                .expect("iterators mutex");
            if let Some(iter) = iters.get_mut(handle_idx) {
                match iter {
                    IteratorState::StringIter { data, byte_pos } => {
                        if *byte_pos < data.len() {
                            // Allocate a string in WASM memory for the current character
                            // For simplicity, return the byte value as a number
                            let byte = data[*byte_pos] as f64;
                            value::encode_f64(byte)
                        } else {
                            value::encode_undefined()
                        }
                    }
                    IteratorState::Error => value::encode_undefined(),
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
            let mut iters = caller
                .data()
                .iterators
                .lock()
                .expect("iterators mutex");
            let done = if let Some(iter) = iters.get_mut(handle_idx) {
                match iter {
                    IteratorState::StringIter { data, byte_pos } => {
                        if *byte_pos >= data.len() {
                            true
                        } else {
                            false
                        }
                    }
                    IteratorState::Error => true,
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
            if value::is_string(val) {
                let ptr = value::decode_string_ptr(val);
                let string_data = read_string_bytes(&mut caller, ptr);
                let len = string_data.len();
                let mut enums = caller
                    .data()
                    .enumerators
                    .lock()
                    .expect("enumerators mutex");
                let handle = enums.len() as u32;
                enums.push(EnumeratorState::StringEnum {
                    length: len,
                    index: 0,
                });
                value::encode_handle(value::TAG_ENUMERATOR, handle)
            } else {
                let mut enums = caller
                    .data()
                    .enumerators
                    .lock()
                    .expect("enumerators mutex");
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
            let mut enums = caller
                .data()
                .enumerators
                .lock()
                .expect("enumerators mutex");
            if let Some(enm) = enums.get_mut(handle_idx) {
                match enm {
                    EnumeratorState::StringEnum { length, index } => {
                        if *index < *length {
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
            let mut enums = caller
                .data()
                .enumerators
                .lock()
                .expect("enumerators mutex");
            if let Some(enm) = enums.get_mut(handle_idx) {
                match enm {
                    EnumeratorState::StringEnum { index, .. } => {
                        // Return the index as a string
                        // For simplicity, return the index as a number for now
                        // TODO: allocate string in WASM memory
                        return value::encode_f64(*index as f64);
                    }
                    EnumeratorState::Error => {}
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
            let mut enums = caller
                .data()
                .enumerators
                .lock()
                .expect("enumerators mutex");
            let done = if let Some(enm) = enums.get_mut(handle_idx) {
                match enm {
                    EnumeratorState::StringEnum { length, index } => *index >= *length,
                    EnumeratorState::Error => true,
                }
            } else {
                true
            };
            value::encode_bool(done)
        },
    );

    let imports = [
        console_log.into(),    // 0
        f64_mod.into(),        // 1
        f64_pow.into(),        // 2
        throw_fn.into(),       // 3
        iterator_from.into(),  // 4
        iterator_next.into(),  // 5
        iterator_close.into(), // 6
        iterator_value.into(), // 7
        iterator_done.into(),  // 8
        enumerator_from.into(),// 9
        enumerator_next.into(),// 10
        enumerator_key.into(), // 11
        enumerator_done.into(),// 12
    ];
    let instance = Instance::new(&mut store, &module, &imports)?;

    let main = instance.get_typed_func::<(), ()>(&mut store, "main")?;
    // Ignore trap from uncaught exceptions (trap is the expected behavior)
    let _ = main.call(&mut store, ());

    drop(store);

    let bytes = output
        .lock()
        .expect("runtime output buffer mutex should not be poisoned")
        .clone();
    let mut writer = writer;
    writer.write_all(&bytes)?;

    Ok(writer)
}

struct RuntimeState {
    output: Arc<Mutex<Vec<u8>>>,
    iterators: Arc<Mutex<Vec<IteratorState>>>,
    enumerators: Arc<Mutex<Vec<EnumeratorState>>>,
}

enum IteratorState {
    StringIter { data: Vec<u8>, byte_pos: usize },
    Error,
}

enum EnumeratorState {
    StringEnum { length: usize, index: usize },
    Error,
}

fn render_value(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Result<String> {
    if value::is_string(val) {
        let ptr = value::decode_string_ptr(val);
        return read_string(caller, ptr);
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
