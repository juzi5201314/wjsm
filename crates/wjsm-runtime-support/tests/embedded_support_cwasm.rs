//! T4.2 验证：当前发布 mark-sweep、G1 与 ZGC embedded support cwasm。
//!
//! 这条测试锁住 support emitter GC flavor 参数化后的核心验收：可用 flavor cwasm
//! 字节有效、wasmtime 可还原、export 集合与 `wjsm-runtime-support::abi::SUPPORT_EXPORTS`
//! 完全一致。

#![cfg(feature = "embedded")]

use wasmparser::{Parser, Payload, TypeRef, ValType};
use wjsm_backend_wasm::{GcFlavor, emit_support_module_managed_heap_v2};
use wjsm_engine_config::EngineConfig;
use wjsm_runtime_support::{
    SupportGcFlavor, abi, embedded_support_cwasm, embedded_support_cwasm_v2,
};

#[test]
fn embedded_available_support_cwasm_is_present_and_nonempty() {
    for flavor in [
        SupportGcFlavor::MarkSweep,
        SupportGcFlavor::G1,
        SupportGcFlavor::Zgc,
    ] {
        let bytes = embedded_support_cwasm(flavor).expect("available cwasm bytes present");
        assert!(
            bytes.len() > 100,
            "{flavor:?} cwasm bytes too small: {} bytes (likely placeholder)",
            bytes.len()
        );
    }
}

#[test]
fn embedded_available_support_flavors_match_abi() {
    assert_eq!(
        abi::AVAILABLE_SUPPORT_GC_FLAVORS,
        &[
            SupportGcFlavor::MarkSweep,
            SupportGcFlavor::G1,
            SupportGcFlavor::Zgc,
        ]
    );
}

#[test]
fn embedded_available_support_cwasm_deserializes() {
    // build.rs 与本测试共用 canonical artifact engine。
    let engine = EngineConfig::artifact().build().expect("engine");

    for flavor in [
        SupportGcFlavor::MarkSweep,
        SupportGcFlavor::G1,
        SupportGcFlavor::Zgc,
    ] {
        let bytes = embedded_support_cwasm(flavor).expect("embedded cwasm");
        let module = unsafe { wasmtime::Module::deserialize(&engine, bytes) }
            .unwrap_or_else(|e| panic!("deserialize {flavor:?} cwasm: {e}"));

        let exports: Vec<&str> = module.exports().map(|e| e.name()).collect();

        for name in abi::SUPPORT_EXPORTS {
            assert!(
                exports.contains(name),
                "{flavor:?} support cwasm 缺少 export: {name}（实际 exports: {exports:?}）"
            );
        }
    }
}

#[test]
fn embedded_available_support_v2_cwasm_deserializes() {
    let engine = EngineConfig::artifact().build().expect("engine");

    for flavor in [
        SupportGcFlavor::MarkSweep,
        SupportGcFlavor::G1,
        SupportGcFlavor::Zgc,
    ] {
        let bytes = embedded_support_cwasm_v2(flavor).expect("embedded V2 cwasm");
        assert!(
            bytes.len() > 100,
            "{flavor:?} V2 cwasm bytes too small: {} bytes (likely placeholder)",
            bytes.len()
        );
        let module = unsafe { wasmtime::Module::deserialize(&engine, bytes) }
            .unwrap_or_else(|error| panic!("deserialize {flavor:?} V2 cwasm: {error}"));
        let exports: Vec<&str> = module.exports().map(|export| export.name()).collect();

        for name in abi::SUPPORT_EXPORTS {
            assert!(
                exports.contains(name),
                "{flavor:?} V2 support cwasm 缺少 export: {name}（实际 exports: {exports:?}）"
            );
        }
    }
}

#[test]
fn support_v2_wasm_imports_memory64_and_heap_globals() {
    for (flavor, emitter_flavor) in [
        (SupportGcFlavor::MarkSweep, GcFlavor::MarkSweep),
        (SupportGcFlavor::G1, GcFlavor::G1),
        (SupportGcFlavor::Zgc, GcFlavor::Zgc),
    ] {
        let wasm =
            emit_support_module_managed_heap_v2(emitter_flavor).expect("managed support V2 wasm");
        let mut heap_memory = None;
        let mut heap_globals = Vec::new();
        for payload in Parser::new(0).parse_all(&wasm) {
            let Payload::ImportSection(imports) = payload.expect("valid support V2 wasm") else {
                continue;
            };
            for import in imports.into_imports() {
                let import = import.expect("valid support V2 import");
                if import.module != abi::ENV_MODULE_NAME {
                    continue;
                }
                if import.name == wjsm_ir::HEAP_MEMORY_NAME {
                    let TypeRef::Memory(memory) = import.ty else {
                        panic!("{flavor:?} V2 heap import is not memory");
                    };
                    heap_memory = Some(memory);
                }
                if abi::MANAGED_HEAP_V2_GLOBAL_IMPORTS.contains(&import.name) {
                    let TypeRef::Global(global) = import.ty else {
                        panic!("{flavor:?} V2 heap global {} has wrong type", import.name);
                    };
                    assert_eq!(global.content_type, ValType::I64);
                    assert!(global.mutable);
                    heap_globals.push(import.name);
                }
            }
        }
        let heap_memory = heap_memory.expect("managed support V2 heap memory import");
        assert!(heap_memory.memory64);
        assert!(heap_memory.shared);
        assert_eq!(heap_memory.initial, wjsm_ir::HEAP_MEMORY_MIN_PAGES);
        assert_eq!(heap_memory.maximum, Some(wjsm_ir::HEAP_MEMORY_MAX_PAGES));
        assert_eq!(heap_globals, abi::MANAGED_HEAP_V2_GLOBAL_IMPORTS);
    }
}
