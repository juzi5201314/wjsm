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

    let view = wjsm_snapshot_format::decode_snapshot(&snapshot_bytes)?;
    let current_abi = wjsm_snapshot_format::abi_hash();
    if view.header.abi_hash != current_abi {
        anyhow::bail!(
            "embedded snapshot ABI hash mismatch: file={:#018x} runtime={:#018x}",
            view.header.abi_hash,
            current_abi
        );
    }

    fs::write(&snapshot_path, &snapshot_bytes)?;

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../wjsm-runtime/src");
    println!("cargo:rerun-if-changed=../wjsm-backend-wasm/src");
    println!("cargo:rerun-if-changed=../wjsm-snapshot-format/src");

    Ok(())
}
