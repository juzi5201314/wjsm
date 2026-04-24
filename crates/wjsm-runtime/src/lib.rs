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

    let mut store = Store::new(
        &engine,
        RuntimeState {
            output: Arc::clone(&output),
        },
    );

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

    let imports = [console_log.into()];
    let instance = Instance::new(&mut store, &module, &imports)?;

    let main = instance.get_typed_func::<(), ()>(&mut store, "main")?;
    main.call(&mut store, ())?;

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
}

fn render_value(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Result<String> {
    if value::is_string(val) {
        let ptr = value::decode_string_ptr(val);
        return read_string(caller, ptr);
    }

    if value::is_undefined(val) {
        return Ok("undefined".to_string());
    }

    Ok(f64::from_bits(val as u64).to_string())
}

fn read_string(caller: &mut Caller<'_, RuntimeState>, ptr: u32) -> Result<String> {
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        anyhow::bail!("Runtime expected exported memory")
    };

    let data = memory.data(caller);
    let start = ptr as usize;
    if start >= data.len() {
        anyhow::bail!("String pointer out of bounds: {ptr}");
    }

    let end = data[start..]
        .iter()
        .position(|byte| *byte == 0)
        .map_or(data.len(), |offset| start + offset);

    Ok(std::str::from_utf8(&data[start..end])?.to_owned())
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
