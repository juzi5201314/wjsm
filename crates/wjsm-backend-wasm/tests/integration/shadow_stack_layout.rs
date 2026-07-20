//! 独立 shadow memory 布局断言：主内存不再预留 shadow/guard；
//! `__shadow_sp=0`，`__shadow_stack_end=INITIAL`。
//! V2 下主内存只承载字符串区，不再 pad 到 V1 object_heap_start。

use std::collections::HashMap;

use wasmparser::{Operator, Parser, Payload};
use wjsm_ir::SHADOW_STACK_INITIAL_SIZE;
#[cfg(not(feature = "managed-heap-v2"))]
use wjsm_ir::constants::{GC_BARRIER_EVENT_BUFFER_SIZE, GC_REGION_SIZE};

#[test]
fn independent_shadow_memory_layout() {
    let wasm = compile("console.log('shadow');");
    let globals = extract_init_globals_i32_sets(&wasm);
    let memories = extract_memory_imports(&wasm);

    let expected_memory_imports = if cfg!(feature = "managed-heap-v2") {
        3
    } else {
        2
    };
    assert_eq!(
        memories.len(),
        expected_memory_imports,
        "user module memory import count differs from selected ABI: {memories:?}"
    );
    assert!(
        memories.iter().any(|(m, n)| m == "env" && n == "memory"),
        "missing env.memory import"
    );
    assert!(
        memories
            .iter()
            .any(|(m, n)| m == "env" && n == wjsm_ir::SHADOW_MEMORY_NAME),
        "missing env.__shadow_memory import"
    );
    #[cfg(feature = "managed-heap-v2")]
    assert!(
        memories
            .iter()
            .any(|(m, n)| m == "env" && n == wjsm_ir::HEAP_MEMORY_NAME),
        "missing env.__heap_memory import"
    );

    let shadow_sp = *globals.get(&4).expect("__shadow_sp (global 4)");
    let shadow_stack_end = *globals
        .get(&7)
        .expect("__shadow_stack_end (global 7) in __wjsm_init_globals");
    let object_heap_start = *globals
        .get(&5)
        .expect("__object_heap_start (global 5) in __wjsm_init_globals");
    let heap_ptr = *globals
        .get(&1)
        .expect("__heap_ptr (global 1) in __wjsm_init_globals");
    let obj_table_ptr = *globals.get(&2).expect("__obj_table_ptr");
    let barrier_buf_ptr = *globals.get(&25).expect("__barrier_buf_ptr");

    assert_eq!(shadow_sp, 0, "independent shadow stack starts at 0");
    assert_eq!(
        shadow_stack_end, SHADOW_STACK_INITIAL_SIZE as i32,
        "shadow_stack_end must be INITIAL capacity"
    );
    assert_eq!(
        heap_ptr, object_heap_start,
        "initial heap_ptr must equal object_heap_start"
    );

    let data = extract_active_data_bytes(&wasm);
    // 不再填充 0xDEADBEEF guard canary。
    assert!(
        !data.windows(4).any(|w| w == [0xDE, 0xAD, 0xBE, 0xEF]),
        "guard canary must not appear in main memory data"
    );

    #[cfg(feature = "managed-heap-v2")]
    {
        // V2：data segment 只含字符串；heap_ptr/object_heap 仍指向预留洞之后。
        assert!(
            barrier_buf_ptr as u32 >= obj_table_ptr as u32,
            "barrier must start at/after handle table base"
        );
        assert!(
            object_heap_start as usize
                >= barrier_buf_ptr as usize
                    + wjsm_ir::constants::GC_BARRIER_EVENT_BUFFER_SIZE as usize,
            "object heap must start after barrier event buffer"
        );
        assert!(
            (obj_table_ptr as usize) >= data.len(),
            "V2 handle table must start at/after compact string data end"
        );
        assert!(
            data.len() < 64 * 1024,
            "V2 data segment must stay compact without V1 object_heap padding, got {}",
            data.len()
        );
        assert!(
            data.len() < object_heap_start as usize,
            "V2 must not embed zero-filled object heap into active data segment"
        );
    }
    #[cfg(not(feature = "managed-heap-v2"))]
    {
        assert!(
            barrier_buf_ptr as u32 >= obj_table_ptr as u32,
            "barrier must start at/after handle table base"
        );
        assert!(
            object_heap_start as usize
                >= barrier_buf_ptr as usize + GC_BARRIER_EVENT_BUFFER_SIZE as usize,
            "object heap must start after barrier event buffer"
        );
        assert_eq!(
            object_heap_start % GC_REGION_SIZE as i32,
            0,
            "object heap must start at a GC region boundary"
        );
        // 主内存 data 段不应再包含 256KiB 影子洞（object_heap 紧跟 barrier 对齐后即可）。
        assert!(
            data.len() >= object_heap_start as usize,
            "data segment must cover up to object heap start"
        );
    }
}

fn compile(source: &str) -> Vec<u8> {
    let module = wjsm_parser::parse_module(source).expect("parse");
    let program = wjsm_semantic::lower_module(module, false).expect("lower");
    wjsm_backend_wasm::compile(&program).expect("compile")
}

fn extract_memory_imports(wasm: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        let Payload::ImportSection(section) = payload.expect("valid wasm") else {
            continue;
        };
        for import in section.into_imports() {
            let import = import.expect("import");
            if matches!(import.ty, wasmparser::TypeRef::Memory(_)) {
                out.push((import.module.to_string(), import.name.to_string()));
            }
        }
        break;
    }
    out
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

/// 解析 `__wjsm_init_globals` 中 `i32.const` + `global.set` 序列。
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
        out.contains_key(&7)
            && out.contains_key(&5)
            && out.contains_key(&1)
            && out.contains_key(&26),
        "init_globals did not set expected layout globals: {out:?}"
    );
    out
}
