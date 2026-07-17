//! Build-time support module precompile.
//!
//! 1. 调用 `wjsm_backend_wasm::emit_support_module(flavor)` 产三种 support.wasm；
//! 2. 用 canonical artifact engine `precompile_module` 预编译为 cwasm；
//! 3. 写入 OUT_DIR/wjsm_support_{mark_sweep,g1,zgc}.cwasm，供 `src/lib.rs`
//!    通过 `include_bytes!` 嵌入二进制。
//!
//! cwasm bytes 是平台/wasmtime 版本敏感的；任何 wasmtime 升级或 backend
//! support_module emit 改动都会触发重建（cargo:rerun-if-changed 已覆盖）。

fn main() -> anyhow::Result<()> {
    if std::env::var_os("CARGO_FEATURE_EMBEDDED").is_none() {
        return Ok(());
    }

    let out_dir =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR not set by cargo"));
    // 唯一 owner：与 runtime deserialize 使用同一 canonical artifact profile。
    let engine = wjsm_engine_config::EngineConfig::artifact().build()?;

    for flavor in [
        wjsm_backend_wasm::GcFlavor::MarkSweep,
        wjsm_backend_wasm::GcFlavor::G1,
        wjsm_backend_wasm::GcFlavor::Zgc,
    ] {
        let suffix = flavor.artifact_suffix();
        let wasm = wjsm_backend_wasm::emit_support_module(flavor)?;
        let cwasm_bytes = engine.precompile_module(&wasm)?;
        std::fs::write(
            out_dir.join(format!("wjsm_support_{suffix}.cwasm")),
            &cwasm_bytes,
        )?;
    }

    // 把 backend support_module emit 与 abi 文件纳入重建链。
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../wjsm-backend-wasm/src/support_module.rs");
    println!("cargo:rerun-if-changed=src/abi.rs");
    println!("cargo:rerun-if-changed=../wjsm-engine-config/src/lib.rs");

    Ok(())
}
