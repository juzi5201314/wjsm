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

/// 按各 active data segment 的偏移重建线性内存映像。
///
/// 产物可能包含**多个** active segment：IC 区是全零的，编译器跳过它不发射
/// （wasm 内存按规范零初始化），于是字符串区被切成「IC 区之前」与「之后」两段。
/// 因此不能只取第一段——必须按每段声明的偏移写入同一张映像，还原真实内存布局。
/// Normal mode 编译 data_base 为 0，故映像下标 = 运行时内存地址。
fn extract_active_data_bytes(wasm: &[u8]) -> Vec<u8> {
    let mut image = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        let wasmparser::Payload::DataSection(section) = payload.expect("valid wasm") else {
            continue;
        };
        for segment_result in section {
            let segment = segment_result.expect("valid segment");
            let wasmparser::DataKind::Active { offset_expr, .. } = segment.kind else {
                continue;
            };
            let mut reader = offset_expr.get_operators_reader();
            let wasmparser::Operator::I32Const { value } =
                reader.read().expect("offset expr operator")
            else {
                panic!("active data segment offset must be i32.const");
            };
            let start = usize::try_from(value).expect("non-negative data offset");
            let end = start + segment.data.len();
            if image.len() < end {
                image.resize(end, 0);
            }
            image[start..end].copy_from_slice(segment.data);
        }
        break;
    }
    image
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
