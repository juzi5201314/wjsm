use std::collections::{HashMap, HashSet};

use wasmparser::{ExternalKind, Operator, Parser, Payload, TypeRef};
use wjsm_backend_wasm::{GcFlavor, emit_support_module};

#[derive(Default)]
struct SupportModuleInfo {
    imported_funcs: Vec<String>,
    exports: HashMap<String, u32>,
    bodies: Vec<Vec<OwnedOperator>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum OwnedOperator {
    Call(u32),
    GlobalGet(u32),
    GlobalSet(u32),
}

fn parse_support_module() -> SupportModuleInfo {
    let wasm = emit_support_module(GcFlavor::MarkSweep).expect("emit support module");
    let mut info = SupportModuleInfo::default();

    for payload in Parser::new(0).parse_all(&wasm) {
        match payload.expect("valid wasm payload") {
            Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    let import = import.expect("valid import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        info.imported_funcs.push(import.name.to_string());
                    }
                }
            }
            Payload::ExportSection(section) => {
                for export in section {
                    let export = export.expect("valid export");
                    if export.kind == ExternalKind::Func {
                        info.exports.insert(export.name.to_string(), export.index);
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                let mut ops = Vec::new();
                let mut reader = body.get_operators_reader().expect("operator reader");
                while !reader.eof() {
                    match reader.read().expect("valid operator") {
                        Operator::Call { function_index } => {
                            ops.push(OwnedOperator::Call(function_index));
                        }
                        Operator::GlobalGet { global_index } => {
                            ops.push(OwnedOperator::GlobalGet(global_index));
                        }
                        Operator::GlobalSet { global_index } => {
                            ops.push(OwnedOperator::GlobalSet(global_index));
                        }
                        _ => {}
                    }
                }
                info.bodies.push(ops);
            }
            _ => {}
        }
    }

    info
}

fn exported_body<'a>(info: &'a SupportModuleInfo, name: &str) -> &'a [OwnedOperator] {
    let func_idx = *info
        .exports
        .get(name)
        .unwrap_or_else(|| panic!("missing support export `{name}`"));
    let body_idx = func_idx
        .checked_sub(info.imported_funcs.len() as u32)
        .unwrap_or_else(|| panic!("support export `{name}` points at imported function"));
    info.bodies
        .get(body_idx as usize)
        .unwrap_or_else(|| panic!("missing body for support export `{name}`"))
}

fn imported_func_index(info: &SupportModuleInfo, name: &str) -> u32 {
    info.imported_funcs
        .iter()
        .position(|import_name| import_name == name)
        .unwrap_or_else(|| panic!("missing imported function `{name}`")) as u32
}

#[test]
fn support_alloc_helpers_use_alloc_window_and_safepoint_poll() {
    const G_HEAP_PTR: u32 = 1;
    const G_ALLOC_PTR: u32 = 19;
    const G_ALLOC_END: u32 = 20;
    const G_GC_ALLOC_BYTES: u32 = 21;
    const G_GC_TRIGGER_BYTES: u32 = 22;

    let info = parse_support_module();
    let gc_alloc_slow_idx = imported_func_index(&info, "gc_alloc_slow");
    let gc_safepoint_poll_idx = imported_func_index(&info, "gc_safepoint_poll");
    let retired_import_name = ["gc", "maybe", "collect"].join("_");
    assert!(
        !info
            .imported_funcs
            .iter()
            .any(|name| name == &retired_import_name),
        "support module must retire old proactive GC import"
    );

    for export in ["obj_new", "arr_new"] {
        let body = exported_body(&info, export);
        let ops: HashSet<_> = body.iter().copied().collect();

        for required in [
            OwnedOperator::GlobalGet(G_ALLOC_PTR),
            OwnedOperator::GlobalGet(G_ALLOC_END),
            OwnedOperator::GlobalSet(G_ALLOC_PTR),
            OwnedOperator::GlobalSet(G_HEAP_PTR),
            OwnedOperator::GlobalGet(G_GC_ALLOC_BYTES),
            OwnedOperator::GlobalSet(G_GC_ALLOC_BYTES),
            OwnedOperator::GlobalGet(G_GC_TRIGGER_BYTES),
            OwnedOperator::Call(gc_safepoint_poll_idx),
            OwnedOperator::Call(gc_alloc_slow_idx),
        ] {
            assert!(
                ops.contains(&required),
                "support `{export}` body missing alloc-window operator {required:?}"
            );
        }
    }
}
