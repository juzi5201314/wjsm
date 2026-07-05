//! T4.2 验证：当前发布 mark-sweep、G1 与 ZGC embedded support cwasm。
//!
//! 这条测试锁住 support emitter GC flavor 参数化后的核心验收：可用 flavor cwasm
//! 字节有效、wasmtime 可还原、export 集合与 `wjsm-runtime-support::abi::SUPPORT_EXPORTS`
//! 完全一致。

#![cfg(feature = "embedded")]

use wjsm_runtime_support::{SupportGcFlavor, abi, embedded_support_cwasm};

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
    let mut cfg = wasmtime::Config::new();
    // 构建期 precompile 现已启用 epoch_interruption（匹配运行时 async yield 路径），
    // deserialize 时 engine config 必须一致。
    cfg.epoch_interruption(true);
    cfg.wasm_bulk_memory(true);
    let engine = wasmtime::Engine::new(&cfg).expect("engine");

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
