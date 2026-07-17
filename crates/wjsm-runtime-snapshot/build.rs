use std::env;
use std::fs;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    if std::env::var_os("CARGO_FEATURE_EMBEDDED").is_none() {
        return Ok(());
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let snapshot_path = out_dir.join("wjsm_startup_snapshot.bin");

    let snapshot_bytes = wjsm_runtime::build_embedded_startup_snapshot_bytes()?;
    if snapshot_bytes.is_empty() {
        anyhow::bail!("embedded startup snapshot is empty");
    }
    wjsm_snapshot_format::decode_snapshot(&snapshot_bytes)
        .map_err(|e| anyhow::anyhow!("embedded startup snapshot self-validation failed: {e:#}"))?;

    fs::write(&snapshot_path, &snapshot_bytes)?;

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
