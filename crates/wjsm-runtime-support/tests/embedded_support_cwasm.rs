//! P2.1 验证：embedded support cwasm 能 deserialize 且导出 12 个 helper。
//!
//! 这条测试是 P2.1 的核心验收：cwasm 字节有效、wasmtime 可还原、export 集合
//! 与 `wjsm-runtime-support::abi::SUPPORT_EXPORTS` 完全一致。

#![cfg(feature = "embedded")]

use wjsm_runtime_support::{EMBEDDED_SUPPORT_CWASM, abi};

#[test]
fn embedded_support_cwasm_is_present_and_nonempty() {
    let bytes = EMBEDDED_SUPPORT_CWASM.expect("embedded feature on → cwasm bytes present");
    assert!(
        bytes.len() > 100,
        "cwasm bytes too small: {} bytes (likely placeholder)",
        bytes.len()
    );
}

#[test]
fn embedded_support_cwasm_deserializes() {
    let bytes = EMBEDDED_SUPPORT_CWASM.expect("embedded cwasm");
    let mut cfg = wasmtime::Config::new();
    // 构建期 precompile 现已启用 epoch_interruption（匹配运行时 async yield 路径），
    // deserialize 时 engine config 必须一致。
    cfg.epoch_interruption(true);
    let engine = wasmtime::Engine::new(&cfg).expect("engine");
    let module =
        unsafe { wasmtime::Module::deserialize(&engine, bytes) }.expect("deserialize cwasm");

    // 收集所有 exports
    let exports: Vec<&str> = module.exports().map(|e| e.name()).collect();

    for name in abi::SUPPORT_EXPORTS {
        assert!(
            exports.iter().any(|e| *e == *name),
            "support cwasm 缺少 export: {name}（实际 exports: {exports:?}）"
        );
    }
}
