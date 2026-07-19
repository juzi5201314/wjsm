use std::env;
use std::fs;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    if std::env::var_os("CARGO_FEATURE_EMBEDDED").is_none() {
        return Ok(());
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let managed_heap_v2 = std::env::var_os("CARGO_FEATURE_MANAGED_HEAP_V2").is_some();
    if !managed_heap_v2 {
        let snapshot_path = out_dir.join("wjsm_startup_snapshot.bin");
        let snapshot_bytes = wjsm_runtime::build_embedded_startup_snapshot_bytes()?;
        if snapshot_bytes.is_empty() {
            anyhow::bail!("embedded startup snapshot is empty");
        }
        wjsm_snapshot_format::decode_snapshot(&snapshot_bytes).map_err(|e| {
            anyhow::anyhow!("embedded startup snapshot self-validation failed: {e:#}")
        })?;
        fs::write(snapshot_path, snapshot_bytes)?;
    }

    if std::env::var_os("CARGO_FEATURE_MANAGED_HEAP_V2").is_some() {
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
        fs::write(artifact_path, artifact_bytes)?;
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
