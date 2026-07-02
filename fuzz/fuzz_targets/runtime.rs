#![no_main]

use libfuzzer_sys::fuzz_target;
use tokio::runtime::Builder;

fn byte_at(data: &[u8], index: usize) -> u8 {
    data.get(index).copied().unwrap_or(0)
}

fn int_at(data: &[u8], index: usize) -> i32 {
    i32::from(byte_at(data, index)) - 128
}

fn string_literal(data: &[u8]) -> String {
    let mut out = String::from("\"");
    for byte in data.iter().copied().take(24) {
        match byte {
            b'\\' => out.push_str("\\\\"),
            b'\"' => out.push_str("\\\""),
            0x20..=0x7e => out.push(byte as char),
            _ => out.push('x'),
        }
    }
    out.push('\"');
    out
}

fn program_from_bytes(data: &[u8]) -> String {
    let a = int_at(data, 1);
    let b = int_at(data, 2);
    let divisor = if b == 0 { 1 } else { b };
    let s = string_literal(data.get(3..).unwrap_or_default());
    match byte_at(data, 0) % 5 {
        0 => format!("console.log(({a}) + ({b})); console.log(({a}) % ({divisor}));"),
        1 => format!("const s = {s}; console.log(s.length); console.log(s + '!');"),
        2 => format!("const x = {a}; console.log(Number('  ' + x + '  ')); console.log(Boolean(x));"),
        3 => format!("const a = [{a}, {b}, {divisor}]; console.log(a.map(x => x + 1).join(','));"),
        _ => format!("function* g() {{ yield {a}; yield {b}; }} let sum = 0; for (const x of g()) sum = sum + x; console.log(sum);"),
    }
}

fuzz_target!(|data: &[u8]| {
    let source = program_from_bytes(data);
    let Ok(wasm) = wjsm_runtime::compile_source(&source) else {
        return;
    };
    let Ok(rt) = Builder::new_current_thread().enable_all().build() else {
        return;
    };
    let _ = rt.block_on(async { wjsm_runtime::execute_with_writer(&wasm, Vec::new()).await });
});
