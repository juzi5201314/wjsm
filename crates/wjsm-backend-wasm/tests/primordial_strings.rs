use wjsm_ir::constants;

#[test]
fn primordial_string_offsets_consistent_across_compilations() {
    let wasm1 = compile("console.log('hello primordial');");
    let wasm2 = compile(r#"const x = "compile2_unique_string_identifier_for_test";"#);

    let data1 = extract_active_data_bytes(&wasm1);
    let data2 = extract_active_data_bytes(&wasm2);

    for (offset, s) in constants::primordial_string_offsets() {
        let needle = s.as_bytes();
        let end = *offset as usize + s.len();
        assert!(
            data1.get(*offset as usize..end) == Some(needle),
            "primordial string \"{s}\" missing/wrong at offset {offset} in compilation 1 (data len={})",
            data1.len(),
        );
        assert!(
            data2.get(*offset as usize..end) == Some(needle),
            "primordial string \"{s}\" missing/wrong at offset {offset} in compilation 2 (data len={})",
            data2.len(),
        );
    }

    assert!(
        find_subslice(&data1, b"hello primordial").is_some(),
        "compilation 1 should embed its user string"
    );
    assert!(
        find_subslice(&data2, b"compile2_unique_string_identifier_for_test").is_some(),
        "compilation 2 should embed its user string"
    );
}

#[test]
fn primordial_strings_start_before_user_region() {
    let wasm = compile("var x = 42;");
    let data = extract_active_data_bytes(&wasm);

    for (offset, s) in constants::primordial_string_offsets() {
        let needle = s.as_bytes();
        let end = *offset as usize + s.len();
        assert!(
            data.get(*offset as usize..end) == Some(needle),
            "primordial string \"{s}\" missing at offset {offset}"
        );
        assert!(
            *offset < constants::USER_STRING_START,
            "offset {offset} >= USER_STRING_START {}",
            constants::USER_STRING_START
        );
    }
}

// ── helpers ──

fn compile(source: &str) -> Vec<u8> {
    let module = wjsm_parser::parse_module(source).expect("parse");
    let program = wjsm_semantic::lower_module(module, false).expect("lower");
    wjsm_backend_wasm::compile(&program).expect("compile")
}

/// 从 WASM 二进制中提取第一个 active data segment 的原始字节内容。
/// Normal mode 编译 data_base 为 0，因此字节偏移 = 运行时内存地址。
fn extract_active_data_bytes(wasm: &[u8]) -> Vec<u8> {
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        let wasmparser::Payload::DataSection(section) = payload.expect("valid wasm") else {
            continue;
        };
        for segment_result in section {
            let segment = segment_result.expect("valid segment");
            if let wasmparser::DataKind::Active { .. } = segment.kind {
                return segment.data.to_vec();
            }
        }
        break;
    }
    Vec::new()
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
