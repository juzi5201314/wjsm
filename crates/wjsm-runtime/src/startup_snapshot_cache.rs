//! Process-wide startup snapshot cache, keyed by cache directory + current ABI hash.
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
    cache_dir_hash: u64,
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

fn cache_file_path() -> PathBuf {
    let mut p = cache_dir();
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let fname = format!(
        "wjsm-startup-snapshot-v{}-{:016x}-{}-{}.bin",
        SNAPSHOT_FORMAT_VERSION,
        abi_hash(),
        std::env::consts::ARCH,
        profile,
    );
    p.push(fname);
    p
}

fn cache_key() -> CacheKey {
    let mut h = DefaultHasher::new();
    cache_dir().hash(&mut h);
    CacheKey {
        cache_dir_hash: h.finish(),
        abi: abi_hash(),
    }
}

/// 校验解码结果与当前 ABI 一致；磁盘载入路径必须经过此检查，内存条目只在 insert 前校验或由本进程 capture 产生。
fn validate_cached_bytes(bytes: &[u8]) -> Option<StartupSnapshotView<'_>> {
    let view = decode_snapshot(bytes).ok()?;
    if view.header.abi_hash != abi_hash() {
        return None;
    }
    Some(view)
}

/// Look up cached primordial snapshot bytes.
pub(crate) async fn get_cached() -> Option<Arc<[u8]>> {
    let key = cache_key();

    {
        let guard = CACHE.lock().await;
        if let Some(map) = &*guard {
            if let Some(bytes) = map.get(&key) {
                return Some(Arc::clone(bytes));
            }
        }
    }

    if let Some(bytes) = read_from_disk() {
        let mut guard = CACHE.lock().await;
        let map = guard.get_or_insert_with(HashMap::new);
        map.insert(key, Arc::clone(&bytes));
        return Some(bytes);
    }

    None
}

/// 丢弃当前 cache 目录（当前 ABI）的内存与磁盘缓存条目。
pub(crate) async fn evict() {
    let key = cache_key();
    {
        let mut guard = CACHE.lock().await;
        if let Some(map) = &mut *guard {
            map.remove(&key);
        }
    }
    let path = cache_file_path();
    let _ = std::fs::remove_file(path);
}

/// Store a newly captured primordial snapshot.
pub(crate) async fn store(bytes: Vec<u8>) {
    let key = cache_key();
    let arc: Arc<[u8]> = Arc::from(bytes.into_boxed_slice());

    let mut guard = CACHE.lock().await;
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert(key, Arc::clone(&arc));
    drop(guard);

    if let Err(e) = write_to_disk(&arc) {
        if crate::startup_snapshot_debug_enabled() {
            eprintln!("startup snapshot cache write failed: {e:#}");
        }
    }
}

fn read_from_disk() -> Option<Arc<[u8]>> {
    let path = cache_file_path();
    let data = std::fs::read(&path).ok()?;
    if validate_cached_bytes(&data).is_none() {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    Some(Arc::from(data.into_boxed_slice()))
}

fn write_to_disk(bytes: &[u8]) -> Result<()> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir)?;
    let final_path = cache_file_path();
    let tmp_path = final_path.with_extension("tmp");
    std::fs::write(&tmp_path, bytes)?;
    std::fs::rename(&tmp_path, &final_path)?;
    Ok(())
}
