//! P1.2: embedded startup snapshot install；运行时不写客户机器磁盘 cache。
use anyhow::Result;
use std::sync::Mutex;
use tokio::runtime::Builder;
use wjsm_runtime::{
    build_embedded_startup_snapshot_bytes, compile_source, embedded_startup_snapshot,
    execute_with_writer, install_embedded_startup_snapshot,
};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn run(source: &str) -> Result<String> {
    let wasm = compile_source(source)?;
    let rt = Builder::new_current_thread().enable_all().build()?;
    let (out, _) = rt.block_on(execute_with_writer(&wasm, Vec::new()))?;
    Ok(String::from_utf8(out)?)
}

#[test]
fn install_embedded_startup_snapshot_exposes_valid_bytes() -> Result<()> {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let bytes = build_embedded_startup_snapshot_bytes()?;
    assert!(!bytes.is_empty());
    install_embedded_startup_snapshot(&bytes);
    let embedded = embedded_startup_snapshot().expect("embedded snapshot 已安装");
    let view = wjsm_snapshot_format::decode_snapshot(embedded)?;
    assert_eq!(view.header.abi_hash, wjsm_snapshot_format::abi_hash());
    Ok(())
}

#[test]
fn embedded_snapshot_first_run_ignores_runtime_cache_env() -> Result<()> {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut probe = std::env::temp_dir();
    probe.push("wjsm-embedded-snapshot-no-cache");
    let _ = std::fs::remove_dir_all(&probe);
    std::fs::create_dir_all(&probe)?;
    unsafe {
        std::env::set_var("WJSM_STARTUP_SNAPSHOT_CACHE", probe.as_os_str());
        std::env::remove_var("WJSM_STARTUP_SNAPSHOT");
    }

    let bytes = build_embedded_startup_snapshot_bytes()?;
    install_embedded_startup_snapshot(&bytes);

    let output = run("console.log(99)")?;
    let wrote_files = std::fs::read_dir(&probe)?
        .any(|entry| entry.map(|entry| entry.path().is_file()).unwrap_or(false));

    unsafe {
        std::env::remove_var("WJSM_STARTUP_SNAPSHOT_CACHE");
    }

    assert_eq!(output, "99\n");
    assert!(!wrote_files, "运行时不允许写 startup snapshot 磁盘 cache");
    Ok(())
}
