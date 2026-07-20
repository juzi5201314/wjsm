use std::env;
use std::fs;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    if std::env::var_os("CARGO_FEATURE_EMBEDDED").is_none() {
        return Ok(());
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let snapshot_feature_v2 = std::env::var_os("CARGO_FEATURE_MANAGED_HEAP_V2").is_some();
    // Feature unification 会把 workspace 内同一 path dep 的 feature 合并。
    // build-dep 的 wjsm-runtime 可能仍是 V1 cfg，但 wjsm-backend-wasm 已被
    // 其它 crate 的 managed-heap-v2 打开。此时默认 `emit_support_module` 若
    // 跟随 backend feature 会产生 V2 support，而 V1 runtime 无法 instantiate。
    // 以 backend feature 状态为准：backend V2 时跳过 V1 snapshot 构建。
    let runtime_v2 = wjsm_runtime::MANAGED_HEAP_V2_ACTIVE;
    let backend_v2 = wjsm_backend_wasm::MANAGED_HEAP_V2_ACTIVE;
    let managed_heap_v2 = snapshot_feature_v2 || runtime_v2 || backend_v2;

    let embeds_path = out_dir.join("embeds.rs");
    if managed_heap_v2 {
        let artifact_path = out_dir.join("wjsm_managed_heap_v2_artifact_abi.bin");
        let engine = wjsm_engine_config::EngineConfig::artifact().build()?;
        let engine_fingerprint = wjsm_engine_config::compatibility_fingerprint(&engine);
        let support_abi_hash = wjsm_runtime_support::abi::managed_heap_v2_support_abi_hash();
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
        fs::write(&artifact_path, artifact_bytes)?;
        fs::write(
            &embeds_path,
            r#"pub static EMBEDDED_STARTUP_SNAPSHOT: Option<&[u8]> = None;
pub static EMBEDDED_MANAGED_HEAP_V2_ARTIFACT_ABI: Option<&[u8]> = Some(include_bytes!(concat!(
    env!("OUT_DIR"),
    "/wjsm_managed_heap_v2_artifact_abi.bin"
)));
"#,
        )?;
    } else {
        let snapshot_path = out_dir.join("wjsm_startup_snapshot.bin");
        let snapshot_bytes = wjsm_runtime::build_embedded_startup_snapshot_bytes()?;
        if snapshot_bytes.is_empty() {
            anyhow::bail!("embedded startup snapshot is empty");
        }
        wjsm_snapshot_format::decode_snapshot(&snapshot_bytes).map_err(|e| {
            anyhow::anyhow!("embedded startup snapshot self-validation failed: {e:#}")
        })?;
        fs::write(snapshot_path, snapshot_bytes)?;
        fs::write(
            &embeds_path,
            r#"pub static EMBEDDED_STARTUP_SNAPSHOT: Option<&[u8]> = Some(include_bytes!(concat!(
    env!("OUT_DIR"),
    "/wjsm_startup_snapshot.bin"
)));
pub static EMBEDDED_MANAGED_HEAP_V2_ARTIFACT_ABI: Option<&[u8]> = None;
"#,
        )?;
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../wjsm-runtime/src");
    println!("cargo:rerun-if-changed=../wjsm-runtime/src/runtime_node_globals.rs");
    println!("cargo:rerun-if-changed=../wjsm-runtime/src/runtime_node_fs.rs");
    println!("cargo:rerun-if-changed=../wjsm-runtime/src/runtime_node_crypto.rs");
    println!("cargo:rerun-if-changed=../wjsm-runtime/src/runtime_node_zlib.rs");
    println!("cargo:rerun-if-changed=../wjsm-runtime/src/runtime_node_data.rs");
    println!("cargo:rerun-if-changed=../wjsm-backend-wasm/src");
    println!("cargo:rerun-if-changed=../wjsm-snapshot-format/src");

    Ok(())
}
