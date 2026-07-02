#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    if source.len() > 16 * 1024 {
        return;
    }

    if let Ok(module) = wjsm_parser::parse_script_as_module(source) {
        let _ = wjsm_semantic::lower_module(module, false);
    }
});
