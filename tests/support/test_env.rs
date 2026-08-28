//! 测试进程的磁盘缓存目录收口。
//!
//! `WJSM_CACHE_DIR` 未设置时缺省回落用户缓存目录（issue #376）；测试进程必须
//! 在首次编译/执行前把缓存重定向到 `/tmp` 下的共享测试缓存，既保持测试产物
//! 落在 `/tmp`，又让同一 frontier / 同一 fixture 的后续用例命中已编好的条目。

use std::path::PathBuf;
use std::sync::Once;

static INIT: Once = Once::new();

/// 与 fixture 套件共享统一缓存根：`/tmp/wjsm-test-cache/native`。
pub fn test_cache_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("wjsm-test-cache").join("native");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// 若外部未显式设置 `WJSM_CACHE_DIR`，把它指向共享测试缓存目录。
pub fn ensure_test_cache_dir() {
    INIT.call_once(|| {
        if std::env::var_os("WJSM_CACHE_DIR").is_none() {
            // SAFETY: 在首个用例执行前设置一次；Once 保证无并发写，后续只读。
            unsafe { std::env::set_var("WJSM_CACHE_DIR", test_cache_dir()) };
        }
    });
}
