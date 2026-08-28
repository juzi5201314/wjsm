//! 模块解析与加载的文件系统 owner。
//!
//! `Disk` 给 `wjsm run` 与打包期读盘；`Recording` 在 Disk 上记录成功读取；
//! `Snapshot` 只服务 packed exe，未命中即 NotFound，不回退磁盘。

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use url::Url;

use crate::bundler::{logical_url_from_path, logical_url_path};
use crate::source_trace::SourceReadTrace;

/// packed exe 的虚拟根与 `file://` 前缀。
pub const SNAPSHOT_VIRTUAL_ROOT: &str = "/wjsm-exec";
pub const SNAPSHOT_FILE_URL_PREFIX: &str = "file:///wjsm-exec/";

/// 模块源码与解析元数据的唯一入口。
#[derive(Clone, Debug)]
pub enum ModuleSourceStore {
    Disk(Arc<DiskSourceStore>),
    Recording(Arc<RecordingSourceStore>),
    Snapshot(Arc<SnapshotSourceStore>),
}

#[derive(Clone, Debug)]
pub struct DiskSourceStore {
    root: PathBuf,
    /// 输入寻址 artifact 缓存的读集追踪（issue #376）；`None` 时零开销。
    trace: Option<Arc<SourceReadTrace>>,
}

#[derive(Debug)]
pub struct RecordingSourceStore {
    disk: DiskSourceStore,
    recorded: Mutex<BTreeMap<String, Vec<u8>>>,
}

#[derive(Clone, Debug)]
pub struct SnapshotSourceStore {
    files: BTreeMap<String, Vec<u8>>,
}

impl DiskSourceStore {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
            trace: None,
        }
    }

    fn with_trace(root: &Path, trace: Arc<SourceReadTrace>) -> Self {
        Self {
            root: root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
            trace: Some(trace),
        }
    }

    fn record_is_file(&self, path: &Path, value: bool) {
        if let Some(trace) = &self.trace {
            trace.record_is_file(path, value);
        }
    }

    fn record_is_dir(&self, path: &Path, value: bool) {
        if let Some(trace) = &self.trace {
            trace.record_is_dir(path, value);
        }
    }

    fn record_canonical(&self, path: &Path, target: &Path) {
        if let Some(trace) = &self.trace {
            trace.record_canonical(path, target);
        }
    }

    fn record_content(&self, path: &Path, bytes: &[u8]) {
        if let Some(trace) = &self.trace {
            trace.record_content(path, bytes);
        }
    }
}

impl RecordingSourceStore {
    pub fn new(root: &Path) -> Self {
        Self {
            disk: DiskSourceStore::new(root),
            recorded: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn recorded_files(&self) -> BTreeMap<String, Vec<u8>> {
        self.recorded
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn record(&self, path: &Path, bytes: &[u8]) {
        if let Ok(logical) = logical_url_from_disk_path(&self.disk.root, path) {
            self.recorded
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(logical, bytes.to_vec());
        }
    }
}

impl SnapshotSourceStore {
    pub fn from_files(files: BTreeMap<String, Vec<u8>>) -> Result<Self> {
        for logical_url in files.keys() {
            validate_snapshot_logical_url(logical_url)?;
        }
        Ok(Self { files })
    }

    pub fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }
}

impl ModuleSourceStore {
    pub fn disk(root: &Path) -> Self {
        Self::Disk(Arc::new(DiskSourceStore::new(root)))
    }

    /// 带读集追踪的磁盘 store：所有读取/探测/canonicalize 结果记入 `trace`，
    /// 供输入寻址 artifact 缓存做命中校验（issue #376）。
    pub fn disk_traced(root: &Path, trace: Arc<SourceReadTrace>) -> Self {
        Self::Disk(Arc::new(DiskSourceStore::with_trace(root, trace)))
    }

    pub fn recording(root: &Path) -> Self {
        Self::Recording(Arc::new(RecordingSourceStore::new(root)))
    }

    pub fn snapshot(files: BTreeMap<String, Vec<u8>>) -> Result<Self> {
        Ok(Self::Snapshot(Arc::new(SnapshotSourceStore::from_files(
            files,
        )?)))
    }

    pub fn root(&self) -> PathBuf {
        match self {
            Self::Disk(store) => store.root.clone(),
            Self::Recording(store) => store.disk.root.clone(),
            Self::Snapshot(_) => snapshot_virtual_root(),
        }
    }

    /// 把入口路径收成 store 内可解析的绝对/虚拟路径。
    pub fn resolve_entry(&self, entry: &Path) -> PathBuf {
        if let Some(logical) = entry.to_str().and_then(logical_from_snapshot_file_url) {
            return snapshot_virtual_path(&logical)
                .unwrap_or_else(|_| snapshot_virtual_root().join(logical));
        }
        match self {
            Self::Snapshot(_) => resolve_snapshot_entry(entry),
            Self::Disk(_) | Self::Recording(_) => {
                if entry.is_absolute() {
                    entry.to_path_buf()
                } else {
                    self.root().join(entry)
                }
            }
        }
    }

    pub fn uses_virtual_identity(&self) -> bool {
        !matches!(self, Self::Disk(_))
    }

    pub fn is_snapshot(&self) -> bool {
        matches!(self, Self::Snapshot(_))
    }

    pub fn recorded_files(&self) -> BTreeMap<String, Vec<u8>> {
        match self {
            Self::Recording(store) => store.recorded_files(),
            Self::Snapshot(store) => store.files.clone(),
            Self::Disk(_) => BTreeMap::new(),
        }
    }

    pub fn include_file(&self, path: &Path) -> Result<(String, Vec<u8>)> {
        let root = self.root();
        let canonical = self.canonicalize(path)?;
        if !self.is_under_root(&canonical) {
            bail!(
                "include path '{}' is outside module root '{}'",
                path.display(),
                root.display()
            );
        }
        let bytes = self.read_to_string(&canonical)?.into_bytes();
        let logical = self.logical_url(&canonical)?;
        if let Self::Recording(store) = self {
            store.record(&canonical, &bytes);
        }
        Ok((logical, bytes))
    }

    /// 把内存中的源文件记入 Recording store，供 `-e` / stdin 打包。
    pub fn record_logical(&self, logical_url: &str, bytes: impl Into<Vec<u8>>) -> Result<()> {
        validate_snapshot_logical_url(logical_url)?;
        match self {
            Self::Recording(store) => {
                store
                    .recorded
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(logical_url.to_string(), bytes.into());
                Ok(())
            }
            Self::Disk(_) | Self::Snapshot(_) => {
                bail!("record_logical requires a recording store")
            }
        }
    }

    pub fn exists(&self, path: &Path) -> bool {
        self.is_file(path) || self.is_dir(path)
    }

    pub fn is_file(&self, path: &Path) -> bool {
        match self {
            Self::Disk(store) => {
                let value = path.is_file();
                store.record_is_file(path, value);
                value
            }
            Self::Recording(_) => path.is_file(),
            Self::Snapshot(store) => snapshot_logical(path)
                .ok()
                .is_some_and(|logical| store.files.contains_key(&logical)),
        }
    }

    pub fn is_dir(&self, path: &Path) -> bool {
        match self {
            Self::Disk(store) => {
                let value = path.is_dir();
                store.record_is_dir(path, value);
                value
            }
            Self::Recording(_) => path.is_dir(),
            Self::Snapshot(store) => snapshot_is_dir(store, path),
        }
    }

    pub fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        match self {
            Self::Disk(store) => {
                let canonical = path
                    .canonicalize()
                    .with_context(|| format!("canonicalize {}", path.display()))?;
                store.record_canonical(path, &canonical);
                Ok(canonical)
            }
            Self::Recording(_) => path
                .canonicalize()
                .with_context(|| format!("canonicalize {}", path.display())),
            Self::Snapshot(store) => snapshot_canonicalize(store, path),
        }
    }

    pub fn read_to_string(&self, path: &Path) -> Result<String> {
        match self {
            Self::Disk(store) => {
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("read {}", path.display()))?;
                store.record_content(path, text.as_bytes());
                Ok(text)
            }
            Self::Recording(store) => {
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("read {}", path.display()))?;
                store.record(path, text.as_bytes());
                Ok(text)
            }
            Self::Snapshot(store) => {
                let logical = snapshot_logical(path)?;
                let bytes = store
                    .files
                    .get(&logical)
                    .ok_or_else(|| anyhow!("snapshot does not contain '{}'", path.display()))?;
                String::from_utf8(bytes.clone())
                    .with_context(|| format!("snapshot file '{logical}' is not UTF-8"))
            }
        }
    }

    pub fn read_bytes(&self, path: &Path) -> Result<Vec<u8>> {
        match self {
            Self::Disk(store) => {
                let bytes =
                    std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
                store.record_content(path, &bytes);
                Ok(bytes)
            }
            Self::Recording(store) => {
                let bytes =
                    std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
                store.record(path, &bytes);
                Ok(bytes)
            }
            Self::Snapshot(store) => {
                let logical = snapshot_logical(path)?;
                store
                    .files
                    .get(&logical)
                    .cloned()
                    .ok_or_else(|| anyhow!("snapshot does not contain '{}'", path.display()))
            }
        }
    }

    /// 列出虚拟目录的直接子名；磁盘 store 返回 `None`，由调用方走 `std::fs`。
    pub fn read_dir_names(&self, path: &Path) -> Result<Option<Vec<String>>> {
        match self {
            Self::Disk(_) | Self::Recording(_) => Ok(None),
            Self::Snapshot(store) => snapshot_read_dir(store, path).map(Some),
        }
    }

    pub fn file_url(&self, path: &Path) -> Result<String> {
        if self.uses_virtual_identity() {
            let logical = self.logical_url(path)?;
            return Ok(snapshot_file_url(&logical));
        }
        Url::from_file_path(path)
            .map(|url| url.to_string())
            .map_err(|()| anyhow!("cannot build file URL for {}", path.display()))
    }

    pub fn logical_url(&self, path: &Path) -> Result<String> {
        match self {
            Self::Disk(store) => logical_url_from_disk_path(&store.root, path),
            Self::Recording(store) => logical_url_from_disk_path(&store.disk.root, path),
            Self::Snapshot(_) => snapshot_logical(path),
        }
    }

    pub fn is_under_root(&self, path: &Path) -> bool {
        match self {
            Self::Disk(store) => path.starts_with(&store.root),
            Self::Recording(store) => path.starts_with(&store.disk.root),
            Self::Snapshot(_) => {
                let normalized = normalize_path(path);
                normalized == snapshot_virtual_root()
                    || normalized == Path::new(SNAPSHOT_VIRTUAL_ROOT)
                    || snapshot_logical(&normalized).is_ok()
            }
        }
    }

    pub fn module_identity(&self, path: &Path) -> Result<(String, String, String)> {
        if self.uses_virtual_identity() {
            let logical = self.logical_url(path)?;
            let filename = format!("{SNAPSHOT_VIRTUAL_ROOT}/{logical}");
            let dirname = filename
                .rsplit_once('/')
                .map(|(parent, _)| parent.to_string())
                .unwrap_or_else(|| SNAPSHOT_VIRTUAL_ROOT.to_string());
            return Ok((filename, dirname, snapshot_file_url(&logical)));
        }
        let filename = path_to_utf8(path)?;
        let dirname = path
            .parent()
            .ok_or_else(|| anyhow!("module path has no parent: {}", path.display()))
            .and_then(path_to_utf8)?;
        let url = Url::from_file_path(path)
            .map_err(|()| {
                anyhow!(
                    "module path cannot be converted to file URL: {}",
                    path.display()
                )
            })?
            .to_string();
        Ok((filename, dirname, url))
    }
}

pub fn snapshot_virtual_root() -> PathBuf {
    PathBuf::from(SNAPSHOT_VIRTUAL_ROOT)
}

pub fn snapshot_file_url(logical_url: &str) -> String {
    format!("{SNAPSHOT_FILE_URL_PREFIX}{logical_url}")
}

pub fn snapshot_virtual_path(logical_url: &str) -> Result<PathBuf> {
    logical_url_path(&snapshot_virtual_root(), logical_url)
}

/// 判断路径是否属于 packed 虚拟根（含 `file:///wjsm-exec/`）。
pub fn is_snapshot_fs_path(path: &Path) -> bool {
    if let Some(text) = path.to_str()
        && logical_from_snapshot_file_url(text).is_some()
    {
        return true;
    }
    let normalized = normalize_path(path);
    normalized == Path::new(SNAPSHOT_VIRTUAL_ROOT)
        || normalized == snapshot_virtual_root()
        || snapshot_logical(&normalized).is_ok()
}

fn validate_snapshot_logical_url(logical_url: &str) -> Result<()> {
    if logical_url.is_empty() {
        bail!("snapshot logical URL is empty");
    }
    let _ = logical_url_path(&snapshot_virtual_root(), logical_url)?;
    Ok(())
}

fn logical_from_snapshot_file_url(value: &str) -> Option<String> {
    value
        .strip_prefix(SNAPSHOT_FILE_URL_PREFIX)
        .filter(|logical| !logical.is_empty())
        .map(str::to_string)
}

fn resolve_snapshot_entry(entry: &Path) -> PathBuf {
    let normalized = normalize_path(entry);
    if snapshot_logical(&normalized).is_ok()
        || normalized == snapshot_virtual_root()
        || normalized == Path::new(SNAPSHOT_VIRTUAL_ROOT)
    {
        return normalized;
    }
    if let Some(text) = entry.to_str()
        && let Ok(path) = snapshot_virtual_path(text)
    {
        return path;
    }
    // `./worker.js` 不能当 logical URL（`.` 不是 Normal 分量），join 后必须再
    // 归一化，否则入口会停在 `/wjsm-exec/./worker.js`，packed worker/fork 的
    // builtin require 会拿到 undefined。
    normalize_path(&snapshot_virtual_root().join(normalized))
}

fn logical_url_from_disk_path(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root).map_err(|_| {
        anyhow!(
            "path {} is outside module root {}",
            path.display(),
            root.display()
        )
    })?;
    logical_url_from_path(relative)
}

fn snapshot_logical(path: &Path) -> Result<String> {
    let normalized = normalize_path(path);
    let relative = normalized
        .strip_prefix(SNAPSHOT_VIRTUAL_ROOT)
        .or_else(|_| normalized.strip_prefix(snapshot_virtual_root()))
        .map_err(|_| {
            anyhow!(
                "path {} is outside {}",
                path.display(),
                SNAPSHOT_VIRTUAL_ROOT
            )
        })?;
    if relative.as_os_str().is_empty() {
        bail!("snapshot path is the virtual root");
    }
    logical_url_from_path(relative)
}

fn snapshot_is_dir(store: &SnapshotSourceStore, path: &Path) -> bool {
    let normalized = normalize_path(path);
    if normalized == Path::new(SNAPSHOT_VIRTUAL_ROOT) || normalized == snapshot_virtual_root() {
        return !store.files.is_empty();
    }
    let Ok(prefix) = snapshot_logical(path) else {
        return false;
    };
    let dir_prefix = format!("{prefix}/");
    store
        .files
        .keys()
        .any(|logical| logical.starts_with(&dir_prefix))
}

fn snapshot_read_dir(store: &SnapshotSourceStore, path: &Path) -> Result<Vec<String>> {
    let normalized = normalize_path(path);
    let prefix = if normalized == Path::new(SNAPSHOT_VIRTUAL_ROOT)
        || normalized == snapshot_virtual_root()
    {
        String::new()
    } else {
        let logical = snapshot_logical(&normalized)?;
        format!("{logical}/")
    };
    let mut names = BTreeMap::new();
    for key in store.files.keys() {
        let remainder = if prefix.is_empty() {
            key.as_str()
        } else {
            match key.strip_prefix(&prefix) {
                Some(rest) => rest,
                None => continue,
            }
        };
        if remainder.is_empty() {
            continue;
        }
        let name = remainder
            .split_once('/')
            .map_or(remainder, |(name, _)| name);
        names.insert(name.to_string(), ());
    }
    if names.is_empty() && !snapshot_is_dir(store, &normalized) {
        bail!("snapshot does not contain directory '{}'", path.display());
    }
    Ok(names.into_keys().collect())
}

fn snapshot_canonicalize(store: &SnapshotSourceStore, path: &Path) -> Result<PathBuf> {
    let normalized = normalize_path(path);
    if snapshot_is_dir(store, &normalized) {
        return Ok(if normalized.as_os_str().is_empty() {
            snapshot_virtual_root()
        } else {
            normalized
        });
    }
    let logical = snapshot_logical(&normalized)?;
    if !store.files.contains_key(&logical) {
        bail!("snapshot does not contain '{logical}'");
    }
    snapshot_virtual_path(&logical)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn path_to_utf8(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("module path is not valid UTF-8: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn scratch() -> PathBuf {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join("wjsm-test-cache")
            .join("source-store")
            .join(format!("{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("scratch");
        path
    }

    #[test]
    fn recording_store_records_successful_reads() {
        let root = scratch();
        fs::write(root.join("main.js"), "export const x = 1;\n").expect("write");
        let store = ModuleSourceStore::recording(&root);
        let text = store.read_to_string(&root.join("main.js")).expect("read");
        assert_eq!(text, "export const x = 1;\n");
        let recorded = store.recorded_files();
        assert_eq!(
            recorded.get("main.js").map(Vec::as_slice),
            Some(b"export const x = 1;\n".as_slice())
        );
    }

    #[test]
    fn snapshot_resolve_entry_normalizes_dot_relative_specs() {
        let mut files = BTreeMap::new();
        files.insert("worker.js".into(), b"export {};\n".to_vec());
        let store = ModuleSourceStore::snapshot(files).expect("snapshot");
        let expected = snapshot_virtual_path("worker.js").expect("path");
        assert_eq!(store.resolve_entry(Path::new("./worker.js")), expected);
        assert_eq!(store.resolve_entry(Path::new("worker.js")), expected);
        assert_eq!(
            store.resolve_entry(Path::new("/wjsm-exec/./worker.js")),
            expected
        );
        assert!(store.is_file(&store.resolve_entry(Path::new("./worker.js"))));
    }

    #[test]
    fn snapshot_store_reads_virtual_paths_and_rejects_misses() {
        let mut files = BTreeMap::new();
        files.insert("main.js".into(), b"export {};\n".to_vec());
        files.insert("dep.js".into(), b"export const v = 1;\n".to_vec());
        let store = ModuleSourceStore::snapshot(files).expect("snapshot");
        let main = snapshot_virtual_path("main.js").expect("path");
        assert!(store.is_file(&main));
        assert!(store.is_dir(&snapshot_virtual_root()));
        assert_eq!(store.read_to_string(&main).expect("read"), "export {};\n");
        assert_eq!(
            store.file_url(&main).expect("url"),
            "file:///wjsm-exec/main.js"
        );
        assert!(
            store
                .read_to_string(&snapshot_virtual_path("missing.js").expect("path"))
                .is_err()
        );
    }

    #[test]
    fn virtual_identity_uses_snapshot_prefix() {
        let root = scratch();
        fs::write(root.join("app.js"), "1\n").expect("write");
        let store = ModuleSourceStore::recording(&root);
        let (filename, dirname, url) = store
            .module_identity(&root.join("app.js"))
            .expect("identity");
        assert_eq!(filename, "/wjsm-exec/app.js");
        assert_eq!(dirname, "/wjsm-exec");
        assert_eq!(url, "file:///wjsm-exec/app.js");
    }

    #[test]
    fn recording_store_records_logical_inline_source() {
        let root = scratch();
        let store = ModuleSourceStore::recording(&root);
        store
            .record_logical("eval.js", b"console.log(1);\n".to_vec())
            .expect("record");
        let recorded = store.recorded_files();
        assert_eq!(
            recorded.get("eval.js").map(Vec::as_slice),
            Some(b"console.log(1);\n".as_slice())
        );
    }
}
