use std::collections::HashMap;

use wasmparser::{Operator, Parser, Payload};
use wjsm_ir::{SHADOW_STACK_HEAP_GUARD_CANARY, SHADOW_STACK_HEAP_GUARD_SIZE, SHADOW_STACK_SIZE};

#[test]
fn shadow_stack_heap_guard_layout_and_canary() {
    let wasm = compile("console.log('guard');");
    let globals = extract_init_globals_i32_sets(&wasm);
    let data = extract_active_data_bytes(&wasm);

    let shadow_stack_end = *globals
        .get(&8)
        .expect("__shadow_stack_end (global 8) in __wjsm_init_globals");
    let object_heap_start = *globals
        .get(&6)
        .expect("__object_heap_start (global 6) in __wjsm_init_globals");
    let heap_ptr = *globals
        .get(&1)
        .expect("__heap_ptr (global 1) in __wjsm_init_globals");

    assert_eq!(
        object_heap_start - shadow_stack_end,
        SHADOW_STACK_HEAP_GUARD_SIZE as i32,
        "object heap must start after a fixed guard past shadow_stack_end"
    );
    assert_eq!(
        heap_ptr, object_heap_start,
        "initial heap_ptr must equal object_heap_start"
    );

    let guard_start = shadow_stack_end as usize;
    let guard_end = object_heap_start as usize;
    assert!(
        guard_end <= data.len(),
        "data segment must cover guard region (len={}, guard_end={})",
        data.len(),
        guard_end
    );
    let guard_slice = &data[guard_start..guard_end];
    assert_eq!(guard_slice.len(), SHADOW_STACK_HEAP_GUARD_SIZE as usize);

    let pattern = SHADOW_STACK_HEAP_GUARD_CANARY;
    for (i, byte) in guard_slice.iter().enumerate() {
        assert_eq!(
            *byte,
            pattern[i % pattern.len()],
            "guard canary mismatch at guard offset {i} (mem {})",
            guard_start + i
        );
    }

    let shadow_sp = *globals.get(&4).expect("__shadow_sp");
    assert_eq!(
        shadow_stack_end - shadow_sp,
        SHADOW_STACK_SIZE as i32,
        "shadow stack span must remain SHADOW_STACK_SIZE"
    );
}

fn compile(source: &str) -> Vec<u8> {
    let module = wjsm_parser::parse_module(source).expect("parse");
    let program = wjsm_semantic::lower_module(module, false).expect("lower");
    wjsm_backend_wasm::compile(&program).expect("compile")
}

fn extract_active_data_bytes(wasm: &[u8]) -> Vec<u8> {
    for payload in Parser::new(0).parse_all(wasm) {
        let Payload::DataSection(section) = payload.expect("valid wasm") else {
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

/// 解析 `__wjsm_init_globals` 中 `i32.const` + `global.set` 序列（按 wasm 字节码，不依赖 WAT 文本）。
fn extract_init_globals_i32_sets(wasm: &[u8]) -> HashMap<u32, i32> {
    let mut import_func_count = 0u32;
    let mut init_globals_func_idx = None;
    let mut code_bodies = Vec::new();

    for payload in Parser::new(0).parse_all(wasm) {
        match payload.expect("valid wasm module") {
            Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    let import = import.expect("import");
                    if matches!(import.ty, wasmparser::TypeRef::Func(_)) {
                        import_func_count += 1;
                    }
                }
            }
            Payload::ExportSection(section) => {
                for export in section {
                    let export = export.expect("export");
                    if export.name == "__wjsm_init_globals"
                        && export.kind == wasmparser::ExternalKind::Func
                    {
                        init_globals_func_idx = Some(export.index);
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                code_bodies.push(body);
            }
            _ => {}
        }
    }

    let func_idx = init_globals_func_idx.expect("missing export __wjsm_init_globals");
    let code_idx = func_idx
        .checked_sub(import_func_count)
        .expect("init_globals must be a defined function") as usize;
    let body = code_bodies
        .get(code_idx)
        .unwrap_or_else(|| panic!("no code body for __wjsm_init_globals (code_idx={code_idx})"));

    let mut reader = body.get_operators_reader().expect("operators");
    let mut pending: Option<i32> = None;
    let mut out = HashMap::new();
    while !reader.eof() {
        let op = reader.read().expect("operator");
        match op {
            Operator::I32Const { value } => pending = Some(value),
            Operator::GlobalSet { global_index } => {
                if let Some(v) = pending.take() {
                    out.insert(global_index, v);
                }
            }
            Operator::I64Const { .. } => pending = None,
            _ => {}
        }
    }
    assert!(
        out.contains_key(&8) && out.contains_key(&6) && out.contains_key(&1),
        "init_globals did not set expected layout globals: {out:?}"
    );
    out
}
