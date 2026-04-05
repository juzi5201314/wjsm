use anyhow::Result;
use wasmtime::*;
use crate::compiler::value;

pub fn execute(wasm_bytes: &[u8]) -> Result<()> {
    let engine = Engine::default();
    let module = Module::new(&engine, wasm_bytes)?;

    let mut store = Store::new(&engine, ());

    let console_log = Func::wrap(&mut store, |mut caller: Caller<'_, ()>, val: i64| {
        if value::is_string(val) {
            let ptr = value::decode_string_ptr(val);
            if let Some(Extern::Memory(mem)) = caller.get_export("memory") {
                let data = mem.data(&caller);
                // Read null-terminated string
                let mut end = ptr as usize;
                while end < data.len() && data[end] != 0 {
                    end += 1;
                }
                if let Ok(s) = std::str::from_utf8(&data[ptr as usize..end]) {
                    println!("{}", s);
                } else {
                    println!("<invalid string>");
                }
            } else {
                println!("<memory error>");
            }
        } else {
            // It's a number (f64 bitcasted to i64)
            let f = f64::from_bits(val as u64);
            println!("{}", f);
        }
    });

    let imports = [console_log.into()];
    let instance = Instance::new(&mut store, &module, &imports)?;

    let main = instance.get_typed_func::<(), ()>(&mut store, "main")?;
    main.call(&mut store, ())?;

    Ok(())
}
