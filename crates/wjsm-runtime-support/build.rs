//! Build-time support module precompile（唯一 V2 ManagedHeap ABI）。
//!
//! 1. 调用 `wjsm_backend_wasm::emit_support_module(flavor)` 产三种 support.wasm；
//! 2. 用 canonical artifact engine `precompile_module` 预编译为 cwasm；
//! 3. 写入 OUT_DIR/wjsm_support_{mark_sweep,g1,zgc}.cwasm。

fn main() -> anyhow::Result<()> {
    if std::env::var_os("CARGO_FEATURE_EMBEDDED").is_none() {
        return Ok(());
    }

    let out_dir =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR not set by cargo"));
    let engine = wjsm_engine_config::EngineConfig::artifact().build()?;
    for flavor in [
        wjsm_backend_wasm::GcFlavor::MarkSweep,
        wjsm_backend_wasm::GcFlavor::G1,
        wjsm_backend_wasm::GcFlavor::Zgc,
    ] {
        let suffix = flavor.artifact_suffix();
        let wasm = wjsm_backend_wasm::emit_support_module(flavor)?;
        wasmparser::Validator::new()
            .validate_all(&wasm)
            .map_err(|error| anyhow::anyhow!("support wasm validation failed: {error:?}"))?;
        let cwasm = engine.precompile_module(&wasm)?;
        std::fs::write(out_dir.join(format!("wjsm_support_{suffix}.cwasm")), &cwasm)?;
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../wjsm-backend-wasm/src/support_module.rs");
    println!("cargo:rerun-if-changed=src/abi.rs");
    println!("cargo:rerun-if-changed=../wjsm-engine-config/src/lib.rs");

    Ok(())
}
