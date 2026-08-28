//! 输入寻址 portable artifact 缓存（issue #376）：
//! `sha256(源码闭包读集 ‖ 编译选项 ‖ 语义 ABI 指纹) → .wjsm`。
//!
//! # 布局
//!
//! `${cache_dir}/artifact/` 下两类文件：
//!
//! - `<content_key>.wjsm` —— 编码后的 portable artifact 原始字节，键即
//!   `sha256(语义 ABI ‖ 选项指纹 ‖ 读集事实)`，可直接 `wjsm run` 调试；
//! - `<index_key>.dep` —— 入口寻址的读集清单（bincode），键为
//!   `sha256(语义 ABI ‖ 选项指纹 ‖ 入口/root 身份)`，内容含读集事实、
//!   module root 与 content key。
//!
//! # 命中流程（不 parse、不 lower）
//!
//! 1. 由入口 canonical 身份 + 选项 + 语义 ABI 算 index key，读 `.dep`；
//! 2. 校验语义 ABI 与选项指纹，逐条回放读集事实（读盘量级）；
//! 3. 由回放通过的事实重算 content key，读 `<content_key>.wjsm` 返回。
//!
//! 任一步失败即 miss，调用方走冷路径重编译并覆盖写入。写入原子
//! （tmp + rename），失败静默——缓存故障绝不导致编译失败。
//!
//! `WJSM_NO_BUILTIN_CACHE` 非空时整体停用：该开关强制非分段 lower 调试路径，
//! artifact 缓存作为 lower 产物缓存必须一并让位。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::builtin_cache::semantic_abi_hash;
use crate::cache_dir::resolve_cache_dir;
use crate::resolution_options::ResolutionOptions;
use crate::source_trace::{SourceFact, SourceReadTrace};

/// 缓存查找/写入的键输入：入口身份 + 解析选项 + 调用方管线盐。
///
/// `pipeline_salt` 由调用方提供，覆盖语义 ABI 之外影响 artifact 字节的输入
/// （CLI 管线源码指纹、script/verify-ir/debug 开关等），整体进入选项指纹。
pub struct ArtifactCacheRequest {
    entry: PathBuf,
    explicit_root: Option<PathBuf>,
    logical_root: Option<PathBuf>,
    options: ResolutionOptions,
    pipeline_salt: Vec<u8>,
}

/// 命中结果：artifact 原始字节与编译时使用的 module root。
pub struct ArtifactCacheHit {
    pub artifact_bytes: Vec<u8>,
    pub module_root: PathBuf,
}

impl ArtifactCacheRequest {
    /// 为文件入口构造键输入；入口与 root 全部 canonical 化。任一路径
    /// 无法 canonical 化或不是 UTF-8 时返回 `None`（放弃缓存，不影响编译）。
    pub fn for_entry(
        entry: &Path,
        explicit_root: Option<&Path>,
        logical_root: Option<&Path>,
        options: &ResolutionOptions,
        pipeline_salt: &[u8],
    ) -> Option<Self> {
        let entry = canonical_utf8(entry)?;
        let explicit_root = match explicit_root {
            Some(root) => Some(canonical_utf8(root)?),
            None => None,
        };
        let logical_root = match logical_root {
            Some(root) => Some(canonical_utf8(root)?),
            None => None,
        };
        Some(Self {
            entry,
            explicit_root,
            logical_root,
            options: options.clone(),
            pipeline_salt: pipeline_salt.to_vec(),
        })
    }

    /// 选项指纹：管线盐 ‖ 解析条件 ‖ 影响 lower 产物的环境开关。
    fn options_fingerprint(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"wjsm-artifact-options-v1\0");
        hash_field(&mut hasher, &self.pipeline_salt);
        hasher.update([u8::from(self.options.browser())]);
        for condition in self.options.conditions() {
            hash_field(&mut hasher, condition.as_bytes());
        }
        hasher.update([0xff]);
        for condition in self
            .options
            .conditions_for_kind(crate::resolution_options::ResolutionKind::Require)
        {
            hash_field(&mut hasher, condition.as_bytes());
        }
        hasher.update([u8::from(wjsm_semantic::licm_disabled_by_env())]);
        hasher.finalize().into()
    }

    /// 入口寻址的 index key（`.dep` 文件名）。
    fn index_key(&self, abi: &[u8; 32], options_fingerprint: &[u8; 32]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"wjsm-artifact-index-v1\0");
        hasher.update(abi);
        hasher.update(options_fingerprint);
        hash_path_field(&mut hasher, Some(&self.entry));
        hash_path_field(&mut hasher, self.explicit_root.as_deref());
        hash_path_field(&mut hasher, self.logical_root.as_deref());
        hasher.finalize().into()
    }
}

/// 查找缓存的 portable artifact；命中返回 artifact 字节与 module root。
pub fn lookup_portable_artifact(request: &ArtifactCacheRequest) -> Option<ArtifactCacheHit> {
    let directory = cache_directory()?;
    lookup_in(&directory, request, &semantic_abi_hash())
}

/// 把一次成功编译的产物写入缓存（best-effort：任何失败静默放弃）。
pub fn store_portable_artifact(
    request: &ArtifactCacheRequest,
    trace: &SourceReadTrace,
    module_root: &Path,
    artifact_bytes: &[u8],
) {
    let Some(directory) = cache_directory() else {
        return;
    };
    if trace.is_unsupported() {
        return;
    }
    let _ = store_in(
        &directory,
        request,
        &semantic_abi_hash(),
        &trace.facts(),
        module_root,
        artifact_bytes,
    );
}

/// artifact 缓存目录：`${resolve_cache_dir()}/artifact`。
/// `WJSM_NO_BUILTIN_CACHE` 非空 → 停用（强制冷 lower 的调试开关）。
fn cache_directory() -> Option<PathBuf> {
    if std::env::var_os("WJSM_NO_BUILTIN_CACHE").is_some() {
        return None;
    }
    Some(resolve_cache_dir()?.join("artifact"))
}

/// `.dep` 载荷：语义 ABI / 选项指纹作废旧条目，facts 供回放，
/// artifact_sha256 防 `.wjsm` 尾部损坏。
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ArtifactDepsFile {
    semantic_abi_hash: [u8; 32],
    options_fingerprint: [u8; 32],
    module_root: String,
    facts: Vec<SourceFact>,
    content_key: [u8; 32],
    artifact_sha256: [u8; 32],
}

fn lookup_in(
    directory: &Path,
    request: &ArtifactCacheRequest,
    abi: &[u8; 32],
) -> Option<ArtifactCacheHit> {
    let options_fingerprint = request.options_fingerprint();
    let index_key = request.index_key(abi, &options_fingerprint);
    let deps_bytes = std::fs::read(deps_path(directory, &index_key)).ok()?;
    let (deps, _): (ArtifactDepsFile, usize) =
        bincode::serde::decode_from_slice(&deps_bytes, bincode::config::standard()).ok()?;
    if deps.semantic_abi_hash != *abi || deps.options_fingerprint != options_fingerprint {
        return None;
    }
    if !deps.facts.iter().all(SourceFact::still_holds) {
        return None;
    }
    // 回放通过后由事实重算 content key：正向映射严格是
    // sha(源码闭包 ‖ 选项 ‖ 语义版本) → .wjsm，.dep 只是加速索引。
    let content_key = content_key(abi, &options_fingerprint, &deps.facts);
    if content_key != deps.content_key {
        return None;
    }
    let artifact_bytes = std::fs::read(artifact_path(directory, &content_key)).ok()?;
    if <[u8; 32]>::from(Sha256::digest(&artifact_bytes)) != deps.artifact_sha256 {
        return None;
    }
    Some(ArtifactCacheHit {
        artifact_bytes,
        module_root: PathBuf::from(deps.module_root),
    })
}

fn store_in(
    directory: &Path,
    request: &ArtifactCacheRequest,
    abi: &[u8; 32],
    facts: &[SourceFact],
    module_root: &Path,
    artifact_bytes: &[u8],
) -> Result<()> {
    let Some(module_root) = module_root.to_str() else {
        bail!("module root 不是 UTF-8，放弃 artifact 缓存");
    };
    std::fs::create_dir_all(directory)
        .with_context(|| format!("创建 artifact 缓存目录 {}", directory.display()))?;
    restrict_directory_permissions(directory);
    let options_fingerprint = request.options_fingerprint();
    let content_key = content_key(abi, &options_fingerprint, facts);
    let deps = ArtifactDepsFile {
        semantic_abi_hash: *abi,
        options_fingerprint,
        module_root: module_root.to_owned(),
        facts: facts.to_vec(),
        content_key,
        artifact_sha256: Sha256::digest(artifact_bytes).into(),
    };
    let deps_bytes = bincode::serde::encode_to_vec(&deps, bincode::config::standard())
        .context("bincode 序列化 artifact 读集清单失败")?;
    // 先写 artifact 再写 .dep：中途失败只留下无索引的 content 文件，
    // 不会出现指向缺失 artifact 的索引。
    write_atomic(
        &artifact_path(directory, &content_key),
        directory,
        artifact_bytes,
    )?;
    write_atomic(
        &deps_path(directory, &request.index_key(abi, &options_fingerprint)),
        directory,
        &deps_bytes,
    )?;
    Ok(())
}

/// content key：正向缓存的输入寻址键。
fn content_key(abi: &[u8; 32], options_fingerprint: &[u8; 32], facts: &[SourceFact]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"wjsm-artifact-content-v1\0");
    hasher.update(abi);
    hasher.update(options_fingerprint);
    for fact in facts {
        match fact {
            SourceFact::IsFile { path, value } => {
                hasher.update([0u8, u8::from(*value)]);
                hash_field(&mut hasher, path.as_bytes());
            }
            SourceFact::IsDir { path, value } => {
                hasher.update([1u8, u8::from(*value)]);
                hash_field(&mut hasher, path.as_bytes());
            }
            SourceFact::Canonical { path, target } => {
                hasher.update([2u8]);
                hash_field(&mut hasher, path.as_bytes());
                hash_field(&mut hasher, target.as_bytes());
            }
            SourceFact::Content { path, sha256 } => {
                hasher.update([3u8]);
                hash_field(&mut hasher, path.as_bytes());
                hasher.update(sha256);
            }
        }
    }
    hasher.finalize().into()
}

fn deps_path(directory: &Path, index_key: &[u8; 32]) -> PathBuf {
    directory.join(format!("{}.dep", hex(index_key)))
}

fn artifact_path(directory: &Path, content_key: &[u8; 32]) -> PathBuf {
    directory.join(format!("{}.wjsm", hex(content_key)))
}

/// 原子写入：同目录临时文件 + rename，避免读到半截文件。
fn write_atomic(path: &Path, directory: &Path, bytes: &[u8]) -> Result<()> {
    static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let counter = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp_path = directory.join(format!(".{}.{counter}.tmp", std::process::id()));
    if let Err(error) = std::fs::write(&tmp_path, bytes) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(error).with_context(|| format!("写入临时缓存文件 {}", tmp_path.display()));
    }
    if let Err(error) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(error).with_context(|| format!("原子替换缓存文件 {}", path.display()));
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_directory_permissions(directory: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn restrict_directory_permissions(_directory: &Path) {}

fn canonical_utf8(path: &Path) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    canonical.to_str()?;
    Some(canonical)
}

fn hash_field(hasher: &mut Sha256, field: &[u8]) {
    let len = u64::try_from(field.len()).expect("缓存键字段长度应可表示为 u64");
    hasher.update(len.to_le_bytes());
    hasher.update(field);
}

fn hash_path_field(hasher: &mut Sha256, path: Option<&Path>) {
    match path.and_then(Path::to_str) {
        Some(text) => {
            hasher.update([1u8]);
            hash_field(hasher, text.as_bytes());
        }
        None => hasher.update([0u8]),
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Scratch {
        project: PathBuf,
        cache: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let base = std::env::temp_dir()
                .join("wjsm-test-cache")
                .join("module")
                .join(format!("artifact-cache-{tag}-{}-{id}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            let project = base.join("project");
            let cache = base.join("cache");
            std::fs::create_dir_all(&project).expect("project dir");
            std::fs::create_dir_all(&cache).expect("cache dir");
            Self { project, cache }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Some(base) = self.project.parent() {
                let _ = std::fs::remove_dir_all(base);
            }
        }
    }

    fn request_for(scratch: &Scratch, salt: &[u8]) -> ArtifactCacheRequest {
        ArtifactCacheRequest::for_entry(
            &scratch.project.join("main.js"),
            None,
            None,
            &ResolutionOptions::default(),
            salt,
        )
        .expect("入口应可 canonical 化")
    }

    fn trace_for(scratch: &Scratch, source: &[u8]) -> SourceReadTrace {
        let trace = SourceReadTrace::default();
        trace.record_content(&scratch.project.join("main.js"), source);
        trace.record_is_file(&scratch.project.join("lib.ts"), false);
        trace
    }

    #[test]
    fn store_then_lookup_roundtrips_without_recompiling() {
        let scratch = Scratch::new("roundtrip");
        let source = b"console.log(1);\n";
        std::fs::write(scratch.project.join("main.js"), source).expect("write entry");

        let request = request_for(&scratch, b"salt");
        let trace = trace_for(&scratch, source);
        let abi = [7u8; 32];
        store_in(
            &scratch.cache,
            &request,
            &abi,
            &trace.facts(),
            &scratch.project,
            b"artifact-bytes",
        )
        .expect("store 应成功");

        let hit = lookup_in(&scratch.cache, &request, &abi).expect("同源同选项应命中");
        assert_eq!(hit.artifact_bytes, b"artifact-bytes");
        assert_eq!(hit.module_root, scratch.project);
    }

    #[test]
    fn semantic_abi_bump_invalidates_entries() {
        let scratch = Scratch::new("abi-bump");
        let source = b"console.log(1);\n";
        std::fs::write(scratch.project.join("main.js"), source).expect("write entry");

        let request = request_for(&scratch, b"salt");
        let trace = trace_for(&scratch, source);
        let abi = [7u8; 32];
        store_in(
            &scratch.cache,
            &request,
            &abi,
            &trace.facts(),
            &scratch.project,
            b"artifact-bytes",
        )
        .expect("store 应成功");

        let mut bumped = abi;
        bumped[0] ^= 1;
        assert!(
            lookup_in(&scratch.cache, &request, &bumped).is_none(),
            "语义 ABI bump 后必须 miss"
        );
        assert!(lookup_in(&scratch.cache, &request, &abi).is_some());
    }

    #[test]
    fn content_edit_invalidates_lookup() {
        let scratch = Scratch::new("content-edit");
        let source = b"console.log(1);\n";
        std::fs::write(scratch.project.join("main.js"), source).expect("write entry");

        let request = request_for(&scratch, b"salt");
        let trace = trace_for(&scratch, source);
        let abi = [7u8; 32];
        store_in(
            &scratch.cache,
            &request,
            &abi,
            &trace.facts(),
            &scratch.project,
            b"artifact-bytes",
        )
        .expect("store 应成功");

        // 入口内容变化 → Content 事实回放失败 → miss。
        std::fs::write(scratch.project.join("main.js"), b"console.log(2);\n").expect("edit");
        assert!(lookup_in(&scratch.cache, &request, &abi).is_none());

        // 恢复原内容 → 重新命中（键是内容寻址，不是 mtime）。
        std::fs::write(scratch.project.join("main.js"), source).expect("restore");
        assert!(lookup_in(&scratch.cache, &request, &abi).is_some());

        // 曾经的否定探测命中新文件 → miss（解析结果可能改变）。
        std::fs::write(scratch.project.join("lib.ts"), b"export {};\n").expect("new probe file");
        assert!(lookup_in(&scratch.cache, &request, &abi).is_none());
    }

    #[test]
    fn pipeline_salt_partitions_the_namespace() {
        let scratch = Scratch::new("salt");
        let source = b"console.log(1);\n";
        std::fs::write(scratch.project.join("main.js"), source).expect("write entry");

        let request = request_for(&scratch, b"salt-a");
        let trace = trace_for(&scratch, source);
        let abi = [7u8; 32];
        store_in(
            &scratch.cache,
            &request,
            &abi,
            &trace.facts(),
            &scratch.project,
            b"artifact-bytes",
        )
        .expect("store 应成功");

        let other = request_for(&scratch, b"salt-b");
        assert!(
            lookup_in(&scratch.cache, &other, &abi).is_none(),
            "不同管线盐（flags / CLI 源码指纹）不得共享条目"
        );
    }

    #[test]
    fn corrupted_artifact_bytes_miss_instead_of_hitting() {
        let scratch = Scratch::new("corrupt");
        let source = b"console.log(1);\n";
        std::fs::write(scratch.project.join("main.js"), source).expect("write entry");

        let request = request_for(&scratch, b"salt");
        let trace = trace_for(&scratch, source);
        let abi = [7u8; 32];
        store_in(
            &scratch.cache,
            &request,
            &abi,
            &trace.facts(),
            &scratch.project,
            b"artifact-bytes",
        )
        .expect("store 应成功");

        // 破坏 content 文件 → artifact_sha256 校验失败 → miss。
        let options_fingerprint = request.options_fingerprint();
        let content = content_key(&abi, &options_fingerprint, &trace.facts());
        std::fs::write(artifact_path(&scratch.cache, &content), b"tampered").expect("corrupt");
        assert!(lookup_in(&scratch.cache, &request, &abi).is_none());
    }

    #[test]
    fn unsupported_trace_is_never_stored() {
        let scratch = Scratch::new("unsupported");
        let source = b"console.log(1);\n";
        std::fs::write(scratch.project.join("main.js"), source).expect("write entry");
        let trace = trace_for(&scratch, source);
        trace.record_is_file(Path::new("relative.js"), true);
        assert!(trace.is_unsupported());
        // store_portable_artifact 对 unsupported trace 直接放弃（走公共入口
        // 需要环境变量，此处只验证判定位；公共入口的门控见 CLI 集成测试）。
    }
}
