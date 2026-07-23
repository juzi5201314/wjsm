use std::env;
use std::fs;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    if std::env::var_os("CARGO_FEATURE_EMBEDDED").is_none() {
        return Ok(());
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    // ManagedHeap V2 是唯一对象堆路径：仅嵌入 V2 artifact ABI，不再构建 V1 startup snapshot。
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
    let embeds_path = out_dir.join("embeds.rs");
    fs::write(
        &embeds_path,
        r#"pub static EMBEDDED_STARTUP_SNAPSHOT: Option<&[u8]> = None;
pub static EMBEDDED_MANAGED_HEAP_V2_ARTIFACT_ABI: Option<&[u8]> = Some(include_bytes!(concat!(
    env!("OUT_DIR"),
    "/wjsm_managed_heap_v2_artifact_abi.bin"
)));
"#,
    )?;

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
