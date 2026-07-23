use std::collections::HashMap;

use wasmparser::{ExternalKind, Operator, Parser, Payload, TypeRef};
use wjsm_backend_wasm::{GcFlavor, emit_support_module};

#[derive(Default)]
struct SupportModuleInfo {
    imported_names: Vec<String>,
    imported_funcs: Vec<String>,
    exports: HashMap<String, u32>,
    bodies: Vec<Vec<OwnedOperator>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum OwnedOperator {
    Call(u32),
    GlobalGet(u32),
    GlobalSet(u32),
    LocalGet(u32),
    LocalSet(u32),
    I32Const(i32),
    I32And,
    I32Or,
    I32Ne,
    I32Add,
    I32Mul,
    I32Shl,
    I32Load,
    I32Store,
    I64Store,
    MemoryCopy,
}

fn parse_support_module_with(flavor: GcFlavor) -> SupportModuleInfo {
    let wasm = emit_support_module(flavor).expect("emit support module");
    let mut info = SupportModuleInfo::default();

    for payload in Parser::new(0).parse_all(&wasm) {
        match payload.expect("valid wasm payload") {
            Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    let import = import.expect("valid import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        info.imported_funcs.push(import.name.to_string());
                    }
                    info.imported_names.push(import.name.to_string());
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
                        Operator::LocalGet { local_index } => {
                            ops.push(OwnedOperator::LocalGet(local_index));
                        }
                        Operator::LocalSet { local_index } => {
                            ops.push(OwnedOperator::LocalSet(local_index));
                        }
                        Operator::I32Const { value } => {
                            ops.push(OwnedOperator::I32Const(value));
                        }
                        Operator::I32And => {
                            ops.push(OwnedOperator::I32And);
                        }
                        Operator::I32Or => {
                            ops.push(OwnedOperator::I32Or);
                        }
                        Operator::I32Ne => {
                            ops.push(OwnedOperator::I32Ne);
                        }
                        Operator::I32Add => {
                            ops.push(OwnedOperator::I32Add);
                        }
                        Operator::I32Mul => {
                            ops.push(OwnedOperator::I32Mul);
                        }
                        Operator::I32Shl => {
                            ops.push(OwnedOperator::I32Shl);
                        }
                        Operator::I32Load { .. } => {
                            ops.push(OwnedOperator::I32Load);
                        }
                        Operator::I32Store { .. } => {
                            ops.push(OwnedOperator::I32Store);
                        }
                        Operator::I64Store { .. } => {
                            ops.push(OwnedOperator::I64Store);
                        }
                        Operator::MemoryCopy { .. } => {
                            ops.push(OwnedOperator::MemoryCopy);
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

fn parse_support_module() -> SupportModuleInfo {
    parse_support_module_with(GcFlavor::MarkSweep)
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

#[test]
fn mark_sweep_support_write_helpers_do_not_emit_barrier_events() {
    let info = parse_support_module();
    // V2 support 写路径透传 host helpers，不再内联 barrier event / flush。
    assert!(
        !info
            .imported_funcs
            .iter()
            .any(|name| name == "gc_barrier_flush"),
        "V2 support must not import gc_barrier_flush"
    );
    for export in ["obj_set", "elem_set"] {
        let body = exported_body(&info, export);
        assert!(
            body.iter().any(|op| matches!(op, OwnedOperator::Call(_))),
            "support `{export}` must call host write helper"
        );
        assert!(
            !body.contains(&OwnedOperator::I32Store),
            "support `{export}` must not write barrier event buffer in main memory"
        );
    }
}

#[test]
fn g1_support_has_no_linear_card_or_region_meta_imports() {
    let info = parse_support_module_with(GcFlavor::G1);

    for retired in ["__card_table_base", "__region_meta_base"] {
        assert!(
            !info.imported_names.iter().any(|name| name == retired),
            "g1 support must not import retired linear metadata global `{retired}`"
        );
    }
}
