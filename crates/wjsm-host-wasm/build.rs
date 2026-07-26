//! 合并自原 `wjsm-runtime-support/build.rs` + `wjsm-runtime-snapshot/build.rs`。
//!
//! 当 `embedded` feature 开启时：
//! 1. 对每个 GC flavor 调 `wjsm_backend_wasm::emit_support_module`，验证后用
//!    canonical artifact engine `precompile_module` 写 `OUT_DIR/wjsm_support_{flavor}.cwasm`；
//! 2. 计算 managed-heap V2 artifact ABI（engine fingerprint + support ABI hash），
//!    自校验后写 `OUT_DIR/wjsm_managed_heap_v2_artifact_abi.bin`；
//! 3. 生成 `OUT_DIR/embeds.rs`（历史保留占位，供潜在 include 使用）。
//!
//! 该 build.rs 直接复用 host-wasm 自身的 `engine_config` 与 `runtime_support::abi`
//! 源码（通过 `#[path]` include），避免把它们做成 build-dependency crate。

// build.rs 只用到共享源码的一小部分符号；dead_code 告警在此无意义。
#[allow(dead_code)]
#[path = "src/engine_config.rs"]
mod engine_config;

#[allow(dead_code)]
#[path = "src/runtime_support/abi.rs"]
mod abi;

fn main() -> anyhow::Result<()> {
    if std::env::var_os("CARGO_FEATURE_EMBEDDED").is_none() {
        return Ok(());
    }

    let out_dir =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR not set by cargo"));

    let engine = engine_config::EngineConfig::artifact().build()?;

    // 1. 预编译三种 support cwasm。
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

    // 2. managed-heap V2 artifact ABI。
    let engine_fingerprint = engine_config::compatibility_fingerprint(&engine);
    let support_abi_hash = abi::managed_heap_v2_support_abi_hash();
    let artifact = wjsm_snapshot_format::ManagedHeapV2ArtifactAbi {
        engine_fingerprint,
        support_abi_hash,
    };
    let artifact_bytes = wjsm_snapshot_format::encode_managed_heap_v2_artifact_abi(artifact);
    wjsm_snapshot_format::decode_managed_heap_v2_artifact_abi(
        &artifact_bytes,
        engine_fingerprint,
        support_abi_hash,
    )
    .map_err(|error| {
        anyhow::anyhow!("managed heap V2 artifact ABI self-validation failed: {error:#}")
    })?;
    std::fs::write(
        out_dir.join("wjsm_managed_heap_v2_artifact_abi.bin"),
        artifact_bytes,
    )?;

    // 3. 历史保留的 embeds.rs 占位（内容已由上面的静态量直接 include_bytes 覆盖）。
    std::fs::write(
        out_dir.join("embeds.rs"),
        "// generated placeholder; artifacts included directly by src/lib.rs\n",
    )?;

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/engine_config.rs");
    println!("cargo:rerun-if-changed=src/runtime_support/abi.rs");
    println!("cargo:rerun-if-changed=../wjsm-backend-wasm/src/support_module.rs");
    println!("cargo:rerun-if-changed=../wjsm-backend-wasm/src");
    println!("cargo:rerun-if-changed=../wjsm-snapshot-format/src");

    Ok(())
}
