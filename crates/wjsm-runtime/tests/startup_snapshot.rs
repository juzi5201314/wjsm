//! Startup snapshot default/on/off 一致性测试；运行时不再持有磁盘 cache 路径。
//!
//! 本模块测试共享 env（WJSM_STARTUP_SNAPSHOT），单测内串行 + 进程内静态 Mutex
//! 保证彼此不抢 env；同时不改 nextest worker 数。

use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokio::runtime::Builder;
use wjsm_runtime::{
    build_embedded_startup_snapshot_bytes, compile_source, execute_with_writer,
    install_embedded_startup_snapshot,
};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn run(source: &str) -> Result<String> {
    let wasm = compile_source(source)?;
    let rt = Builder::new_current_thread().enable_all().build()?;
    let out = rt.block_on(async { execute_with_writer(&wasm, Vec::new()).await })?;
    Ok(String::from_utf8(out)?)
}

const FIXTURE: &str = r#"
const arr = [1, 2, 3];
console.log(arr.map(x => x * 2).join("-"));
console.log(arr.push(4), arr.length);
console.log(arr.reduce((s, x) => s + x, 0));

function f(x) { return x + 1; }
console.log(f.name, f(41));
Object.defineProperty(f, "custom", { value: 99, configurable: true });
console.log(f.custom);

const obj = { a: 1, b: 2 };
console.log(Object.keys(obj).join(","), JSON.stringify(obj));
"#;

const EXPECTED: &str = "2-4-6\n4 4\n10\nf 42\n99\na,b {\"a\":1,\"b\":2}\n";

fn isolated_dir(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("wjsm-snap-test-{}-{}", tag, std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).expect("mkdir cache probe dir");
    p
}

fn collect_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    entries.sort();
    Ok(entries)
}

#[test]
fn startup_snapshot_default_on_does_not_write_runtime_cache() -> Result<()> {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let probe = isolated_dir("default-on-no-cache");
    unsafe {
        std::env::set_var("WJSM_STARTUP_SNAPSHOT_CACHE", probe.as_os_str());
        std::env::remove_var("WJSM_STARTUP_SNAPSHOT");
    }

    let output = run("console.log(3)")?;
    let files = collect_files(&probe)?;

    unsafe {
        std::env::remove_var("WJSM_STARTUP_SNAPSHOT_CACHE");
    }

    assert_eq!(output, "3\n");
    assert!(
        files.is_empty(),
        "runtime startup snapshot must not write cache files: {files:?}"
    );
    Ok(())
}

#[test]
fn startup_snapshot_explicit_off_values_keep_output() -> Result<()> {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for value in ["0", "false", "off"] {
        unsafe {
            std::env::set_var("WJSM_STARTUP_SNAPSHOT", value);
        }

        let output = run("console.log(4)")?;
        assert_eq!(output, "4\n");
    }
    unsafe {
        std::env::remove_var("WJSM_STARTUP_SNAPSHOT");
    }
    Ok(())
}

#[test]
fn startup_snapshot_off_on_parity_without_runtime_cache() -> Result<()> {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        std::env::set_var("WJSM_STARTUP_SNAPSHOT", "0");
    }
    let off_output = run(FIXTURE)?;
    assert_eq!(off_output, EXPECTED);

    unsafe {
        std::env::set_var("WJSM_STARTUP_SNAPSHOT", "1");
    }
    let on_first = run(FIXTURE)?;
    let on_second = run(FIXTURE)?;
    unsafe {
        std::env::remove_var("WJSM_STARTUP_SNAPSHOT");
    }

    assert_eq!(off_output, on_first, "off vs on-first mismatch");
    assert_eq!(off_output, on_second, "off vs on-second mismatch");
    Ok(())
}

#[test]
fn embedded_snapshot_rebases_array_methods_between_modules() -> Result<()> {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let bytes = build_embedded_startup_snapshot_bytes()?;
    install_embedded_startup_snapshot(&bytes);
    unsafe {
        std::env::set_var("WJSM_STARTUP_SNAPSHOT", "1");
    }

    let output = run(r#"
function a(x) { return x; }
function b(x) { return x; }
function c(x) { return x; }
function d(x) { return x; }
function e(x) { return x; }
const arr = [1, 2, 3];
console.log(arr.push(4), arr.length);
"#)?;

    unsafe {
        std::env::remove_var("WJSM_STARTUP_SNAPSHOT");
    }

    assert_eq!(output, "4 4\n");
    Ok(())
}
