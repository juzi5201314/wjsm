//! 输入寻址 artifact 缓存的读集追踪（issue #376）。
//!
//! 一次 lower 过程中，磁盘 store 观察到的全部文件系统事实都记录在
//! [`SourceReadTrace`]：成功读取的内容哈希、`is_file`/`is_dir` 存在性探测
//! （含否定探测，覆盖扩展名试探等解析路径）与 `canonicalize` 结果（覆盖
//! 符号链接改指向）。缓存命中校验按事实逐条回放：任一事实不再成立即 miss，
//! 保证「源码闭包」键既覆盖被读到的文件，也覆盖影响解析结果的缺失探测。
//!
//! builtin 虚拟路径（`/__wjsm_builtin__/node/...`）不入读集：builtin 源码
//! 已由语义 ABI 指纹覆盖。非 UTF-8 或相对路径无法可靠序列化/回放，遇到即
//! 标记整条 trace 不可用，调用方放弃缓存写入（宁可 miss 不可脏命中）。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

use crate::builtin_modules;

/// 单条文件系统事实：路径 + 观察值。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum SourceFact {
    /// `is_file(path)` 的观察结果（含否定探测）。
    IsFile { path: String, value: bool },
    /// `is_dir(path)` 的观察结果（含否定探测）。
    IsDir { path: String, value: bool },
    /// `canonicalize(path)` 的成功结果（覆盖符号链接改指向）。
    Canonical { path: String, target: String },
    /// 成功读取的文件内容 SHA-256。
    Content { path: String, sha256: [u8; 32] },
}

impl SourceFact {
    /// 事实是否仍然成立（用 `std::fs` 直接回放，不经 store）。
    pub fn still_holds(&self) -> bool {
        match self {
            Self::IsFile { path, value } => Path::new(path).is_file() == *value,
            Self::IsDir { path, value } => Path::new(path).is_dir() == *value,
            Self::Canonical { path, target } => Path::new(path)
                .canonicalize()
                .is_ok_and(|canonical| canonical == Path::new(target)),
            Self::Content { path, sha256 } => std::fs::read(path)
                .is_ok_and(|bytes| <[u8; 32]>::from(Sha256::digest(&bytes)) == *sha256),
        }
    }
}

/// 事实键：同一路径的同类事实只记首次观察（编译期间文件被并发改写本身
/// 就是竞态，首次观察与实际使用的内容一致的概率最高）。
type FactKey = (u8, String);

fn fact_key(fact: &SourceFact) -> FactKey {
    match fact {
        SourceFact::IsFile { path, .. } => (0, path.clone()),
        SourceFact::IsDir { path, .. } => (1, path.clone()),
        SourceFact::Canonical { path, .. } => (2, path.clone()),
        SourceFact::Content { path, .. } => (3, path.clone()),
    }
}

/// 一次 lower 的读集；线程安全（worker 线程可共享 store 克隆）。
#[derive(Debug, Default)]
pub struct SourceReadTrace {
    facts: Mutex<BTreeMap<FactKey, SourceFact>>,
    unsupported: AtomicBool,
}

impl SourceReadTrace {
    pub fn record_is_file(&self, path: &Path, value: bool) {
        if let Some(path) = self.traceable_path(path) {
            self.insert(SourceFact::IsFile { path, value });
        }
    }

    pub fn record_is_dir(&self, path: &Path, value: bool) {
        if let Some(path) = self.traceable_path(path) {
            self.insert(SourceFact::IsDir { path, value });
        }
    }

    pub fn record_canonical(&self, path: &Path, target: &Path) {
        let Some(path) = self.traceable_path(path) else {
            return;
        };
        let Some(target) = self.traceable_path(target) else {
            return;
        };
        self.insert(SourceFact::Canonical { path, target });
    }

    pub fn record_content(&self, path: &Path, bytes: &[u8]) {
        if let Some(path) = self.traceable_path(path) {
            self.insert(SourceFact::Content {
                path,
                sha256: Sha256::digest(bytes).into(),
            });
        }
    }

    /// 读集是否含无法序列化/回放的路径；为真时调用方必须放弃缓存写入。
    pub fn is_unsupported(&self) -> bool {
        self.unsupported.load(Ordering::Relaxed)
    }

    /// 按 (kind, path) 确定序导出全部事实。
    pub fn facts(&self) -> Vec<SourceFact> {
        self.facts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect()
    }

    fn insert(&self, fact: SourceFact) {
        self.facts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(fact_key(&fact))
            .or_insert(fact);
    }

    /// builtin 虚拟路径不入读集；相对/非 UTF-8 路径标记 trace 不可用。
    fn traceable_path(&self, path: &Path) -> Option<String> {
        if builtin_modules::is_builtin_virtual_path(path) {
            return None;
        }
        if !path.is_absolute() {
            self.unsupported.store(true, Ordering::Relaxed);
            return None;
        }
        match path.to_str() {
            Some(text) => Some(text.to_owned()),
            None => {
                self.unsupported.store(true, Ordering::Relaxed);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("wjsm-test-cache")
            .join("module")
            .join(format!("source-trace-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn facts_replay_against_filesystem() {
        let dir = scratch("replay");
        let file = dir.join("main.js");
        std::fs::write(&file, b"console.log(1);\n").expect("write");

        let trace = SourceReadTrace::default();
        trace.record_content(&file, b"console.log(1);\n");
        trace.record_is_file(&file, true);
        trace.record_is_file(&dir.join("missing.ts"), false);
        trace.record_is_dir(&dir, true);
        trace.record_canonical(&file, &file.canonicalize().expect("canonicalize"));

        let facts = trace.facts();
        assert_eq!(facts.len(), 5);
        assert!(facts.iter().all(SourceFact::still_holds));

        // 内容改变 → Content 事实失效。
        std::fs::write(&file, b"console.log(2);\n").expect("rewrite");
        assert!(!facts.iter().all(SourceFact::still_holds));
        std::fs::write(&file, b"console.log(1);\n").expect("restore");

        // 否定探测命中新文件 → IsFile(false) 失效。
        std::fs::write(dir.join("missing.ts"), b"export {};\n").expect("new file");
        assert!(!facts.iter().all(SourceFact::still_holds));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn builtin_virtual_paths_are_excluded() {
        let trace = SourceReadTrace::default();
        trace.record_is_file(Path::new("/__wjsm_builtin__/node/fs.mjs"), true);
        assert!(trace.facts().is_empty());
        assert!(!trace.is_unsupported());
    }

    #[test]
    fn relative_paths_mark_trace_unsupported() {
        let trace = SourceReadTrace::default();
        trace.record_is_file(Path::new("relative.js"), true);
        assert!(trace.facts().is_empty());
        assert!(trace.is_unsupported());
    }

    #[test]
    fn first_observation_wins_per_path_and_kind() {
        let trace = SourceReadTrace::default();
        trace.record_content(Path::new("/a.js"), b"first");
        trace.record_content(Path::new("/a.js"), b"second");
        let facts = trace.facts();
        assert_eq!(facts.len(), 1);
        assert_eq!(
            facts[0],
            SourceFact::Content {
                path: "/a.js".into(),
                sha256: Sha256::digest(b"first").into(),
            }
        );
    }
}
