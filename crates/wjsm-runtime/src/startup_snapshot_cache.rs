//! Process-wide startup snapshot cache, keyed by wasm bytes hash + current ABI hash.
//!
//! First run of a module: executes __wjsm_bootstrap_once + host post-bootstrap + capture.
//! Subsequent runs: restore from cached snapshot, skipping bootstrap.
//! Disk cache uses atomic rename for safe concurrent access.

use anyhow::Result;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::startup_snapshot_format::*;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct CacheKey {
    wasm_hash: u64,
    abi: u64,
}

static CACHE: Mutex<Option<HashMap<CacheKey, Arc<[u8]>>>> = Mutex::const_new(None);

fn cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("WJSM_STARTUP_SNAPSHOT_CACHE") {
        return PathBuf::from(dir);
    }
    let mut p = std::env::temp_dir();
    p.push("wjsm");
    p.push("startup-snapshot");
    p
}

fn cache_file_path(wasm_hash: u64) -> PathBuf {
    let mut p = cache_dir();
    let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
    let fname = format!(
        "wjsm-startup-snapshot-v{}-{:016x}-{:016x}-{}-{}.bin",
        SNAPSHOT_FORMAT_VERSION,
        abi_hash(),
        wasm_hash,
        std::env::consts::ARCH,
        profile,
    );
    p.push(fname);
    p
}

pub(crate) fn wasm_bytes_hash(wasm: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    wasm.hash(&mut h);
    h.finish()
}

fn cache_key(wasm_bytes: &[u8]) -> CacheKey {
    CacheKey {
        wasm_hash: wasm_bytes_hash(wasm_bytes),
        abi: abi_hash(),
    }
}

/// 校验解码结果与当前 ABI 一致；内存/磁盘命中路径均须经过此检查。
fn validate_cached_bytes(bytes: &[u8]) -> Option<StartupSnapshotView<'_>> {
    let view = decode_snapshot(bytes).ok()?;
    if view.header.abi_hash != abi_hash() {
        return None;
    }
    Some(view)
}

/// Look up cached snapshot bytes for this module.
pub(crate) async fn get_cached(wasm_bytes: &[u8]) -> Option<Arc<[u8]>> {
    let key = cache_key(wasm_bytes);

    {
        let guard = CACHE.lock().await;
        if let Some(map) = &*guard {
            if let Some(bytes) = map.get(&key) {
                if validate_cached_bytes(bytes).is_some() {
                    return Some(Arc::clone(bytes));
                }
            }
        }
    }

    if let Some(bytes) = read_from_disk(key.wasm_hash) {
        let mut guard = CACHE.lock().await;
        let map = guard.get_or_insert_with(HashMap::new);
        map.insert(key, Arc::clone(&bytes));
        return Some(bytes);
    }

    None
}

/// 丢弃当前模块（wasm hash + 当前 ABI）的内存与磁盘缓存条目。
pub(crate) async fn evict(wasm_bytes: &[u8]) {
    let key = cache_key(wasm_bytes);
    {
        let mut guard = CACHE.lock().await;
        if let Some(map) = &mut *guard {
            map.remove(&key);
        }
    }
    let path = cache_file_path(key.wasm_hash);
    let _ = std::fs::remove_file(path);
}

/// Store a newly captured snapshot for this module.
pub(crate) async fn store(wasm_bytes: &[u8], bytes: Vec<u8>) {
    let key = cache_key(wasm_bytes);
    let arc: Arc<[u8]> = Arc::from(bytes.into_boxed_slice());

    let mut guard = CACHE.lock().await;
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert(key, Arc::clone(&arc));
    drop(guard);

    let _ = write_to_disk(key.wasm_hash, &arc);
}

fn read_from_disk(wasm_hash: u64) -> Option<Arc<[u8]>> {
    let path = cache_file_path(wasm_hash);
    let data = std::fs::read(&path).ok()?;
    if validate_cached_bytes(&data).is_none() {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    Some(Arc::from(data.into_boxed_slice()))
}

fn write_to_disk(wasm_hash: u64, bytes: &[u8]) -> Result<()> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir)?;
    let final_path = cache_file_path(wasm_hash);
    let tmp_path = final_path.with_extension("tmp");
    std::fs::write(&tmp_path, bytes)?;
    std::fs::rename(&tmp_path, &final_path)?;
    Ok(())
}