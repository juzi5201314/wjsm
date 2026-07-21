use std::collections::HashMap;
#[cfg(not(feature = "managed-heap-v2"))]
use std::collections::HashSet;

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

fn imported_func_index(info: &SupportModuleInfo, name: &str) -> u32 {
    info.imported_funcs
        .iter()
        .position(|import_name| import_name == name)
        .unwrap_or_else(|| panic!("missing imported function `{name}`")) as u32
}

#[cfg(not(feature = "managed-heap-v2"))]
fn contains_subsequence(body: &[OwnedOperator], needle: &[OwnedOperator]) -> bool {
    body.windows(needle.len()).any(|window| window == needle)
}

#[cfg(not(feature = "managed-heap-v2"))]
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

#[cfg(not(feature = "managed-heap-v2"))]
#[test]
fn support_obj_set_re_resolves_old_ptr_before_resize_copy() {
    const G_OBJ_TABLE_PTR: u32 = 2;

    let info = parse_support_module();
    let body = exported_body(&info, "obj_set");

    let re_resolve_old_ptr = [
        OwnedOperator::GlobalGet(G_OBJ_TABLE_PTR),
        OwnedOperator::LocalGet(9),
        OwnedOperator::I32Const(2),
        OwnedOperator::I32Shl,
        OwnedOperator::I32Add,
        OwnedOperator::I32Load,
        OwnedOperator::LocalSet(6),
        OwnedOperator::LocalGet(8),
        OwnedOperator::LocalGet(6),
        OwnedOperator::LocalGet(4),
        OwnedOperator::I32Const(32),
        OwnedOperator::I32Mul,
        OwnedOperator::I32Const(16),
        OwnedOperator::I32Add,
        OwnedOperator::MemoryCopy,
    ];
    assert!(
        contains_subsequence(body, &re_resolve_old_ptr),
        "support obj_set must reload old_ptr from obj_table immediately before resize memory.copy"
    );
}

#[cfg(not(feature = "managed-heap-v2"))]
#[test]
fn g1_support_write_helpers_emit_barrier_events() {
    const G_BARRIER_BUF_PTR: u32 = 25;
    const G_BARRIER_BUF_END: u32 = 26;
    const EVENT_SIZE: i32 = 24;

    let info = parse_support_module_with(GcFlavor::G1);
    let barrier_flush_idx = imported_func_index(&info, "gc_barrier_flush");

    for export in ["obj_set", "elem_set"] {
        let body = exported_body(&info, export);
        let ops: HashSet<_> = body.iter().copied().collect();
        for required in [
            OwnedOperator::GlobalGet(G_BARRIER_BUF_PTR),
            OwnedOperator::GlobalGet(G_BARRIER_BUF_END),
            OwnedOperator::GlobalSet(G_BARRIER_BUF_PTR),
            OwnedOperator::I32Const(EVENT_SIZE),
            OwnedOperator::Call(barrier_flush_idx),
        ] {
            assert!(
                ops.contains(&required),
                "g1 support `{export}` body missing barrier event operator {required:?}"
            );
        }
    }
}

#[cfg(not(feature = "managed-heap-v2"))]
#[test]
fn zgc_support_read_helpers_emit_load_barriers() {
    const G_GOOD_COLOR: u32 = 24;

    let info = parse_support_module_with(GcFlavor::Zgc);
    let load_barrier_idx = imported_func_index(&info, "gc_load_barrier_slow");

    for export in ["obj_get", "obj_set", "obj_delete", "elem_get", "elem_set"] {
        let body = exported_body(&info, export);
        let ops: HashSet<_> = body.iter().copied().collect();
        for required in [
            OwnedOperator::GlobalGet(G_GOOD_COLOR),
            OwnedOperator::I32Const(3),
            OwnedOperator::I32Const(-4),
            OwnedOperator::I32And,
            OwnedOperator::I32Ne,
            OwnedOperator::Call(load_barrier_idx),
        ] {
            assert!(
                ops.contains(&required),
                "zgc support `{export}` body missing load-barrier operator {required:?}"
            );
        }
        assert!(
            contains_subsequence(
                body,
                &[
                    OwnedOperator::I32Const(3),
                    OwnedOperator::I32And,
                    OwnedOperator::GlobalGet(G_GOOD_COLOR),
                    OwnedOperator::I32Ne,
                ],
            ),
            "zgc support `{export}` must compare entry color against __good_color"
        );
    }
}

#[cfg(not(feature = "managed-heap-v2"))]
#[test]
fn zgc_support_write_helpers_emit_satb_events_without_satb_ptr() {
    const G_GC_PHASE: u32 = 23;
    const G_BARRIER_BUF_PTR: u32 = 25;
    const G_BARRIER_BUF_END: u32 = 26;
    const EVENT_SIZE: i32 = 24;

    let info = parse_support_module_with(GcFlavor::Zgc);
    let barrier_flush_idx = imported_func_index(&info, "gc_barrier_flush");

    assert!(
        !info.imported_names.iter().any(|name| name == "__satb_ptr"),
        "zgc support must reuse the unified barrier buffer, not import __satb_ptr"
    );

    for export in ["obj_set", "elem_set"] {
        let body = exported_body(&info, export);
        let ops: HashSet<_> = body.iter().copied().collect();
        for required in [
            OwnedOperator::GlobalGet(G_GC_PHASE),
            OwnedOperator::GlobalGet(G_BARRIER_BUF_PTR),
            OwnedOperator::GlobalGet(G_BARRIER_BUF_END),
            OwnedOperator::GlobalSet(G_BARRIER_BUF_PTR),
            OwnedOperator::I32Const(1),
            OwnedOperator::I32Const(EVENT_SIZE),
            OwnedOperator::I32Store,
            OwnedOperator::I64Store,
            OwnedOperator::Call(barrier_flush_idx),
        ] {
            assert!(
                ops.contains(&required),
                "zgc support `{export}` body missing SATB event operator {required:?}"
            );
        }
    }
}

#[cfg(not(feature = "managed-heap-v2"))]
#[test]
fn zgc_alloc_helpers_store_current_good_color_in_obj_table_entries() {
    const G_OBJ_TABLE_PTR: u32 = 2;
    const G_GOOD_COLOR: u32 = 24;

    let info = parse_support_module_with(GcFlavor::Zgc);

    for export in ["obj_new", "arr_new"] {
        let body = exported_body(&info, export);
        assert!(
            contains_subsequence(
                body,
                &[
                    OwnedOperator::GlobalGet(G_OBJ_TABLE_PTR),
                    OwnedOperator::I32Add,
                    OwnedOperator::LocalGet(2),
                    OwnedOperator::GlobalGet(G_GOOD_COLOR),
                    OwnedOperator::I32Or,
                    OwnedOperator::I32Store,
                ],
            ),
            "zgc support `{export}` must write ptr | __good_color to obj_table"
        );
    }
}

#[test]
fn mark_sweep_support_write_helpers_do_not_emit_barrier_events() {
    let info = parse_support_module();
    let barrier_flush_idx = imported_func_index(&info, "gc_barrier_flush");

    for export in ["obj_set", "elem_set"] {
        let body = exported_body(&info, export);
        assert!(
            !body.contains(&OwnedOperator::Call(barrier_flush_idx)),
            "mark-sweep support `{export}` must not call gc_barrier_flush"
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
