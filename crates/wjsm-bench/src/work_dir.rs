//! 路径与工作目录约定。

use std::path::PathBuf;

/// 仓库根：由 CARGO_MANIFEST_DIR 编译期推导，与 cwd 无关。
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/wjsm-bench 的 parent 是 crates/")
        .parent()
        .expect("crates/ 的 parent 是仓库根")
        .to_path_buf()
}

/// 场景目录：`<root>/bench/scenarios`。
pub fn scenarios_dir() -> PathBuf {
    repo_root().join("bench").join("scenarios")
}

/// 临时工作目录（hyperfine 中间 JSON 等），固定路径，可安全删除。
pub fn work_dir() -> PathBuf {
    std::env::temp_dir().join("wjsm-bench-work")
}

/// cold 档每轮测量前清空重建的 wjsm 编译缓存目录。
pub const COLD_CACHE_DIR: &str = "/tmp/wjsm-bench-cold-cache";
