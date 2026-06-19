//! Startup snapshot on/off 一致性测试 + cache 行为专项测试。
//!
//! 三个测试共享 env（WJSM_STARTUP_SNAPSHOT[/_CACHE]），单测内串行 + 进程内静态 Mutex
//! 保证彼此不抢 env；同时不改 nextest worker 数。

use anyhow::Result;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tokio::runtime::Runtime;
use wjsm_runtime::{compile_source, execute_with_writer};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn run(source: &str) -> Result<String> {
    let wasm = compile_source(source)?;
    let rt = Runtime::new()?;
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

fn isolated_cache_dir(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("wjsm-snap-test-{}-{}", tag, std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).expect("mkdir cache");
    p
}

#[test]
fn startup_snapshot_off_on_warm_parity() -> Result<()> {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let cache = isolated_cache_dir("parity");
    // SAFETY: 进程内 ENV_LOCK 串行；外层不会并行改这两个 env。
    unsafe {
        std::env::set_var("WJSM_STARTUP_SNAPSHOT_CACHE", cache.as_os_str());
        std::env::set_var("WJSM_STARTUP_SNAPSHOT", "0");
    }
    let off_output = run(FIXTURE)?;
    assert_eq!(off_output, EXPECTED);

    unsafe {
        std::env::set_var("WJSM_STARTUP_SNAPSHOT", "1");
    }
    let on_cold = run(FIXTURE)?;
    let on_warm = run(FIXTURE)?;
    unsafe {
        std::env::remove_var("WJSM_STARTUP_SNAPSHOT");
        std::env::remove_var("WJSM_STARTUP_SNAPSHOT_CACHE");
    }

    assert_eq!(off_output, on_cold, "off vs on-cold mismatch");
    assert_eq!(off_output, on_warm, "off vs on-warm(restore) mismatch");
    Ok(())
}

#[test]
fn snapshot_cache_hit_persists_file_between_runs() -> Result<()> {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let cache = isolated_cache_dir("hit");
    unsafe {
        std::env::set_var("WJSM_STARTUP_SNAPSHOT_CACHE", cache.as_os_str());
        std::env::set_var("WJSM_STARTUP_SNAPSHOT", "1");
    }

    let _ = run("console.log(1+1)")?;

    let collect = || -> Result<Vec<(PathBuf, Vec<u8>)>> {
        let mut entries: Vec<(PathBuf, Vec<u8>)> = fs::read_dir(&cache)?
            .filter_map(|e| e.ok())
            .map(|e| (e.path(), fs::read(e.path()).unwrap_or_default()))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(entries)
    };

    let before = collect()?;
    assert!(!before.is_empty(), "first run must produce snapshot file");

    let _ = run("console.log(1+1)")?;
    let after = collect()?;

    unsafe {
        std::env::remove_var("WJSM_STARTUP_SNAPSHOT");
        std::env::remove_var("WJSM_STARTUP_SNAPSHOT_CACHE");
    }

    assert_eq!(before.len(), after.len(), "warm run must not add files");
    for ((p1, b1), (p2, b2)) in before.iter().zip(after.iter()) {
        assert_eq!(p1, p2);
        assert_eq!(b1, b2, "warm run must not rewrite cache bytes");
    }
    Ok(())
}

#[test]
fn startup_snapshot_cache_rebases_array_methods_between_modules() -> Result<()> {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let cache = isolated_cache_dir("cross-module");
    unsafe {
        std::env::set_var("WJSM_STARTUP_SNAPSHOT_CACHE", cache.as_os_str());
        std::env::set_var("WJSM_STARTUP_SNAPSHOT", "1");
    }

    let _ = run("")?;
    let cache_files_after_seed = fs::read_dir(&cache)?.count();
    let output = run(r#"
function a(x) { return x; }
function b(x) { return x; }
function c(x) { return x; }
function d(x) { return x; }
function e(x) { return x; }
const arr = [1, 2, 3];
console.log(arr.push(4), arr.length);
"#)?;
    let cache_files_after_user = fs::read_dir(&cache)?.count();

    unsafe {
        std::env::remove_var("WJSM_STARTUP_SNAPSHOT");
        std::env::remove_var("WJSM_STARTUP_SNAPSHOT_CACHE");
    }

    assert_eq!(output, "4 4\n");
    assert_eq!(
        cache_files_after_seed, 1,
        "seed run must create one snapshot"
    );
    assert_eq!(
        cache_files_after_seed, cache_files_after_user,
        "user module must reuse the seed snapshot file"
    );
    Ok(())
}
