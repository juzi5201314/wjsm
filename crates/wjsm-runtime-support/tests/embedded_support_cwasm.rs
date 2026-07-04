//! P1.5 验证：当前仅发布 mark-sweep embedded support cwasm。
//!
//! 这条测试锁住 support emitter GC flavor 参数化后的核心验收：mark-sweep cwasm
//! 字节有效、wasmtime 可还原、export 集合与 `wjsm-runtime-support::abi::SUPPORT_EXPORTS`
//! 完全一致；G1/ZGC 在对应阶段前不得产出伪 artifact。

#![cfg(feature = "embedded")]

use wjsm_runtime_support::{SupportGcFlavor, abi, embedded_support_cwasm};

#[test]
fn embedded_mark_sweep_support_cwasm_is_present_and_nonempty() {
    let bytes = embedded_support_cwasm(SupportGcFlavor::MarkSweep)
        .expect("embedded feature on → mark-sweep cwasm bytes present");
    assert!(
        bytes.len() > 100,
        "mark-sweep cwasm bytes too small: {} bytes (likely placeholder)",
        bytes.len()
    );
}

#[test]
fn unsupported_embedded_support_flavors_are_absent() {
    for flavor in [SupportGcFlavor::G1, SupportGcFlavor::Zgc] {
        assert!(embedded_support_cwasm(flavor).is_none());
    }
}

#[test]
fn embedded_mark_sweep_support_cwasm_deserializes() {
    let mut cfg = wasmtime::Config::new();
    // 构建期 precompile 现已启用 epoch_interruption（匹配运行时 async yield 路径），
    // deserialize 时 engine config 必须一致。
    cfg.epoch_interruption(true);
    cfg.wasm_bulk_memory(true);
    let engine = wasmtime::Engine::new(&cfg).expect("engine");

    let bytes = embedded_support_cwasm(SupportGcFlavor::MarkSweep).expect("embedded cwasm");
    let module = unsafe { wasmtime::Module::deserialize(&engine, bytes) }
        .unwrap_or_else(|e| panic!("deserialize mark-sweep cwasm: {e}"));

    let exports: Vec<&str> = module.exports().map(|e| e.name()).collect();

    for name in abi::SUPPORT_EXPORTS {
        assert!(
            exports.contains(name),
            "mark-sweep support cwasm 缺少 export: {name}（实际 exports: {exports:?}）"
        );
    }
}
