use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use sha2::{Digest, Sha256};
use thiserror::Error;
use wjsm_artifact_format::PortableArtifact;

use crate::image::{CompiledImage, ImageLoadError};
use crate::{
    CRANELIFT_VERSION, NATIVE_CODEGEN_HASH, NativeCompileError, NativeCompiler, NativeObject,
    NativeSymbolResolver,
};

const CACHE_MAGIC: &[u8; 8] = b"WJSMNAT\0";
const CACHE_SCHEMA: u32 = 4;
const MAX_CACHE_OBJECT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CACHE_FUNCTIONS: u32 = 4_000_000;
/// IC 槽上限：每槽 16 字节，4M 槽即 64 MiB 缓冲，远超任何真实程序。
const MAX_CACHE_IC_SLOTS: u32 = 4_000_000;
/// 反馈槽上限：4M 槽 × 48 字节 = 192 MiB，防御恶意/损坏条目。
const MAX_CACHE_FEEDBACK_SLOTS: u32 = 4_000_000;
/// 自动淘汰上限：缓存总字节数超过该值后按 mtime 删最旧条目。
/// 可用 `WJSM_CACHE_MAX_BYTES` 覆盖；`0` 表示禁用自动淘汰。
const DEFAULT_CACHE_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// 每写入多少条目检查一次目录大小（避免每次 store 都全目录扫描）。
const LRU_CHECK_INTERVAL: u64 = 32;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NativeCacheKey {
    pub program_digest: [u8; 32],
    pub native_abi_hash: [u8; 32],
    pub codegen_hash: [u8; 32],
    pub target: Arc<str>,
    pub cranelift_version: Arc<str>,
    pub settings: Arc<str>,
}

impl NativeCacheKey {
    pub fn for_program(program: &wjsm_ir::Program, compiler: &NativeCompiler) -> Self {
        Self {
            program_digest: program_digest(program),
            native_abi_hash: wjsm_native_abi::native_abi_hash(),
            codegen_hash: NATIVE_CODEGEN_HASH,
            target: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS).into(),
            cranelift_version: CRANELIFT_VERSION.into(),
            settings: compiler.settings_key().into(),
        }
    }

    pub fn new(artifact: &PortableArtifact, compiler: &NativeCompiler) -> Self {
        Self::for_program(artifact.program(), compiler)
    }

    pub fn digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(CACHE_SCHEMA.to_le_bytes());
        hasher.update(self.program_digest);
        hasher.update(self.native_abi_hash);
        hasher.update(self.codegen_hash);
        hash_string(&mut hasher, &self.target);
        hash_string(&mut hasher, &self.cranelift_version);
        hash_string(&mut hasher, &self.settings);
        hasher.finalize().into()
    }

    fn image_id(&self) -> u64 {
        u64::from_le_bytes(
            self.digest()[..8]
                .try_into()
                .expect("digest prefix has fixed width"),
        )
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NativeCacheStats {
    pub entries: u64,
    pub bytes: u64,
    pub hits: u64,
    pub misses: u64,
    pub invalidated: u64,
}

#[derive(Default)]
struct AtomicCacheStats {
    hits: AtomicU64,
    misses: AtomicU64,
    invalidated: AtomicU64,
}

/// 写入计数：用于节流 LRU 目录扫描（static 计数跨 repository 共享，
/// 但淘汰本身按 directory 独立执行，仅用于降低扫描频率）。
static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct NativeImageRepository {
    compiler: NativeCompiler,
    cache_dir: Option<PathBuf>,
    state: Arc<Mutex<RepositoryState>>,
    stats: AtomicCacheStats,
}

#[derive(Default)]
struct RepositoryState {
    /// 弱引用条目：调用方持有的 `Arc` 决定 image 生命周期；没有 owner 的 image
    /// 可被回收，再次 prepare 时重新编译/重新读盘。overlay 永不进入 repository。
    images: HashMap<NativeCacheKey, std::sync::Weak<CompiledImage>>,
    inflight: HashMap<NativeCacheKey, Arc<InflightGate>>,
}

#[derive(Default)]
struct InflightGate {
    done: Mutex<bool>,
    ready: Condvar,
}

static SHARED_IMAGE_STATE: std::sync::LazyLock<Arc<Mutex<RepositoryState>>> =
    std::sync::LazyLock::new(|| Arc::new(Mutex::new(RepositoryState::default())));

impl NativeImageRepository {
    pub fn new(compiler: NativeCompiler, cache_dir: Option<PathBuf>) -> Self {
        let state = if cache_dir.is_some() {
            Arc::new(Mutex::new(RepositoryState::default()))
        } else {
            Arc::clone(&SHARED_IMAGE_STATE)
        };
        Self {
            compiler,
            cache_dir,
            state,
            stats: AtomicCacheStats::default(),
        }
    }

    pub fn prepare(
        &self,
        artifact: &PortableArtifact,
        resolver: &dyn NativeSymbolResolver,
    ) -> Result<Arc<CompiledImage>, NativeCacheError> {
        self.prepare_program(artifact.program(), resolver)
    }

    pub fn prepare_program(
        &self,
        program: &wjsm_ir::Program,
        resolver: &dyn NativeSymbolResolver,
    ) -> Result<Arc<CompiledImage>, NativeCacheError> {
        let slots = crate::lower::slots_from_program(program)?;
        self.prepare_program_with_slots(program, &slots, resolver)
    }

    pub fn prepare_program_with_slots(
        &self,
        program: &wjsm_ir::Program,
        variable_slots: &HashMap<String, u32>,
        resolver: &dyn NativeSymbolResolver,
    ) -> Result<Arc<CompiledImage>, NativeCacheError> {
        let key = NativeCacheKey::for_program(program, &self.compiler);
        loop {
            let (gate, leader) = {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(image) = state.images.get(&key).and_then(std::sync::Weak::upgrade) {
                    self.stats.hits.fetch_add(1, Ordering::Relaxed);
                    return Ok(image);
                }
                // 上一个 owner 已释放：清除失效弱引用，按 miss 重编译。
                state.images.remove(&key);
                if let Some(gate) = state.inflight.get(&key) {
                    (Arc::clone(gate), false)
                } else {
                    let gate = Arc::new(InflightGate::default());
                    state.inflight.insert(key.clone(), Arc::clone(&gate));
                    (gate, true)
                }
            };
            if !leader {
                let mut done = gate
                    .done
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                while !*done {
                    done = gate
                        .ready
                        .wait(done)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                continue;
            }

            let prepared = self.prepare_leader(&key, program, variable_slots, resolver);
            {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.inflight.remove(&key);
                if let Ok(image) = &prepared {
                    state.images.insert(key.clone(), Arc::downgrade(image));
                }
            }
            let mut done = gate
                .done
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *done = true;
            gate.ready.notify_all();
            return prepared;
        }
    }

    /// 用打包期预编译的 object 发布 image，不走 compile / 磁盘 cache。
    pub fn load_precompiled(
        &self,
        program: &wjsm_ir::Program,
        object: &NativeObject,
        resolver: &dyn NativeSymbolResolver,
    ) -> Result<Arc<CompiledImage>, NativeCacheError> {
        let key = NativeCacheKey::for_program(program, &self.compiler);
        let expected_feedback = crate::lower::feedback_site_count(program);
        if object.feedback_slot_count() != expected_feedback {
            return Err(NativeCacheError::Invalid(
                "precompiled object feedback slot count does not match program".into(),
            ));
        }
        let function_count = u32::try_from(program.functions().len())
            .map_err(|_| NativeCacheError::LengthOverflow)?;
        if object.function_count() != function_count {
            return Err(NativeCacheError::Invalid(
                "precompiled object function count does not match program".into(),
            ));
        }
        let image = CompiledImage::load(object, key.image_id(), resolver)?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.images.insert(key, Arc::downgrade(&image));
        Ok(image)
    }

    pub fn stats(&self) -> NativeCacheStats {
        let (entries, bytes) = self
            .cache_dir
            .as_deref()
            .and_then(cache_dir_stats)
            .unwrap_or((0, 0));
        NativeCacheStats {
            entries,
            bytes,
            hits: self.stats.hits.load(Ordering::Relaxed),
            misses: self.stats.misses.load(Ordering::Relaxed),
            invalidated: self.stats.invalidated.load(Ordering::Relaxed),
        }
    }

    fn prepare_leader(
        &self,
        key: &NativeCacheKey,
        program: &wjsm_ir::Program,
        variable_slots: &HashMap<String, u32>,
        resolver: &dyn NativeSymbolResolver,
    ) -> Result<Arc<CompiledImage>, NativeCacheError> {
        if let Some(directory) = &self.cache_dir {
            match load_cache_entry(directory, key) {
                Ok(Some(object)) => {
                    // 缓存命中仍按当前 Program 重算反馈槽数并校验：槽编号是
                    // base image 与特化 overlay 的共享契约，不一致即视为损坏。
                    let expected_feedback = crate::lower::feedback_site_count(program);
                    let loaded = if object.feedback_slot_count() == expected_feedback {
                        CompiledImage::load(&object, key.image_id(), resolver)
                    } else {
                        Err(ImageLoadError::InvalidFeedbackSlotCount)
                    };
                    match loaded {
                        Ok(image) => {
                            self.stats.hits.fetch_add(1, Ordering::Relaxed);
                            return Ok(image);
                        }
                        Err(_) => {
                            self.invalidate(directory, key);
                        }
                    }
                }
                Ok(None) => {}
                Err(_) => self.invalidate(directory, key),
            }
        }
        self.stats.misses.fetch_add(1, Ordering::Relaxed);
        let object = self
            .compiler
            .compile_program_with_slots(program, variable_slots)?;
        let image = CompiledImage::load(&object, key.image_id(), resolver)?;
        if let Some(directory) = &self.cache_dir {
            store_cache_entry(directory, key, &object)?;
        }
        Ok(image)
    }

    fn invalidate(&self, directory: &Path, key: &NativeCacheKey) {
        self.stats.invalidated.fetch_add(1, Ordering::Relaxed);
        let _ = fs::remove_file(cache_path(directory, key));
    }
}

fn cache_path(directory: &Path, key: &NativeCacheKey) -> PathBuf {
    directory.join(format!("{}.wnat", hex(&key.digest())))
}

fn load_cache_entry(
    directory: &Path,
    key: &NativeCacheKey,
) -> Result<Option<NativeObject>, NativeCacheError> {
    let path = cache_path(directory, key);
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    validate_cache_permissions(&file)?;
    let length = file.metadata()?.len();
    if length > MAX_CACHE_OBJECT_BYTES {
        return Err(NativeCacheError::Invalid(
            "cache entry exceeds byte limit".into(),
        ));
    }
    let capacity = usize::try_from(length).map_err(|_| NativeCacheError::LengthOverflow)?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes)?;
    decode_cache_entry(&bytes, key).map(Some)
}

fn store_cache_entry(
    directory: &Path,
    key: &NativeCacheKey,
    object: &NativeObject,
) -> Result<(), NativeCacheError> {
    fs::create_dir_all(directory)?;
    set_directory_permissions(directory)?;
    let bytes = encode_cache_entry(key, object)?;
    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let final_path = cache_path(directory, key);
    let temp_path = directory.join(format!(".{}.{}.tmp", std::process::id(), counter));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    set_file_create_permissions(&mut options);
    let mut file = options.open(&temp_path)?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp_path, &final_path)?;
        File::open(directory)?.sync_all()?;
        Ok::<_, std::io::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result?;
    maybe_evict_lru(directory);
    Ok(())
}

fn encode_cache_entry(
    key: &NativeCacheKey,
    object: &NativeObject,
) -> Result<Vec<u8>, NativeCacheError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(CACHE_MAGIC);
    bytes.extend_from_slice(&CACHE_SCHEMA.to_le_bytes());
    bytes.extend_from_slice(&key.program_digest);
    bytes.extend_from_slice(&key.native_abi_hash);
    bytes.extend_from_slice(&key.codegen_hash);
    encode_string(&mut bytes, &key.target)?;
    encode_string(&mut bytes, &key.cranelift_version)?;
    encode_string(&mut bytes, &key.settings)?;
    bytes.extend_from_slice(&sha256(object.bytes()));
    bytes.extend_from_slice(
        &u64::try_from(object.bytes().len())
            .map_err(|_| NativeCacheError::LengthOverflow)?
            .to_le_bytes(),
    );
    bytes.extend_from_slice(&object.function_count().to_le_bytes());
    for frame in object.frame_bytes() {
        bytes.extend_from_slice(&frame.to_le_bytes());
    }
    bytes.extend_from_slice(&object.ic_slot_count().to_le_bytes());
    bytes.extend_from_slice(&object.feedback_slot_count().to_le_bytes());
    bytes.extend_from_slice(object.bytes());
    Ok(bytes)
}

fn decode_cache_entry(
    bytes: &[u8],
    expected: &NativeCacheKey,
) -> Result<NativeObject, NativeCacheError> {
    let mut decoder = CacheDecoder::new(bytes);
    if decoder.take(8)? != CACHE_MAGIC {
        return Err(NativeCacheError::Invalid("cache magic mismatch".into()));
    }
    if decoder.u32()? != CACHE_SCHEMA {
        return Err(NativeCacheError::Invalid("cache schema mismatch".into()));
    }
    let key = NativeCacheKey {
        program_digest: decoder.hash()?,
        native_abi_hash: decoder.hash()?,
        codegen_hash: decoder.hash()?,
        target: decoder.string()?.into(),
        cranelift_version: decoder.string()?.into(),
        settings: decoder.string()?.into(),
    };
    if &key != expected {
        return Err(NativeCacheError::Invalid("cache key mismatch".into()));
    }
    let expected_object_hash = decoder.hash()?;
    let object_len = decoder.u64()?;
    if object_len > MAX_CACHE_OBJECT_BYTES {
        return Err(NativeCacheError::Invalid(
            "cached object exceeds byte limit".into(),
        ));
    }
    let function_count = decoder.u32()?;
    if function_count > MAX_CACHE_FUNCTIONS {
        return Err(NativeCacheError::Invalid(
            "cached function count exceeds limit".into(),
        ));
    }
    let count = usize::try_from(function_count).map_err(|_| NativeCacheError::LengthOverflow)?;
    let mut frame_bytes = Vec::with_capacity(count);
    for _ in 0..count {
        frame_bytes.push(decoder.u32()?);
    }
    let ic_slot_count = decoder.u32()?;
    if ic_slot_count > MAX_CACHE_IC_SLOTS {
        return Err(NativeCacheError::Invalid(
            "cached ic slot count exceeds limit".into(),
        ));
    }
    let feedback_slot_count = decoder.u32()?;
    if feedback_slot_count > MAX_CACHE_FEEDBACK_SLOTS {
        return Err(NativeCacheError::Invalid(
            "cached feedback slot count exceeds limit".into(),
        ));
    }
    let object_len = usize::try_from(object_len).map_err(|_| NativeCacheError::LengthOverflow)?;
    let object: Arc<[u8]> = decoder.take(object_len)?.into();
    decoder.finish()?;
    if sha256(&object) != expected_object_hash {
        return Err(NativeCacheError::Invalid(
            "cached object hash mismatch".into(),
        ));
    }
    Ok(NativeObject {
        bytes: object,
        frame_bytes,
        function_count,
        ic_slot_count,
        feedback_slot_count,
    })
}

struct CacheDecoder<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> CacheDecoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], NativeCacheError> {
        let end = self
            .cursor
            .checked_add(len)
            .ok_or(NativeCacheError::LengthOverflow)?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .ok_or_else(|| NativeCacheError::Invalid("cache entry is truncated".into()))?;
        self.cursor = end;
        Ok(value)
    }

    fn u32(&mut self) -> Result<u32, NativeCacheError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("fixed-width field"),
        ))
    }

    fn u64(&mut self) -> Result<u64, NativeCacheError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("fixed-width field"),
        ))
    }

    fn hash(&mut self) -> Result<[u8; 32], NativeCacheError> {
        Ok(self.take(32)?.try_into().expect("fixed-width hash"))
    }

    fn string(&mut self) -> Result<String, NativeCacheError> {
        let len = usize::try_from(self.u32()?).map_err(|_| NativeCacheError::LengthOverflow)?;
        let bytes = self.take(len)?;
        Ok(std::str::from_utf8(bytes)
            .map_err(|_| NativeCacheError::Invalid("cache string is not UTF-8".into()))?
            .to_owned())
    }

    fn finish(self) -> Result<(), NativeCacheError> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(NativeCacheError::Invalid(
                "cache entry has trailing bytes".into(),
            ))
        }
    }
}

fn encode_string(bytes: &mut Vec<u8>, value: &str) -> Result<(), NativeCacheError> {
    bytes.extend_from_slice(
        &u32::try_from(value.len())
            .map_err(|_| NativeCacheError::LengthOverflow)?
            .to_le_bytes(),
    );
    bytes.extend_from_slice(value.as_bytes());
    Ok(())
}

fn program_digest(program: &wjsm_ir::Program) -> [u8; 32] {
    let bytes = wjsm_artifact_format::encode_program_bytes(program)
        .expect("verified/lowered Program 必须能编码");
    Sha256::digest(&bytes).into()
}

fn hash_string(hasher: &mut Sha256, value: &str) {
    hasher.update(
        u64::try_from(value.len())
            .expect("cache key string length fits u64")
            .to_le_bytes(),
    );
    hasher.update(value.as_bytes());
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

fn cache_dir_stats(directory: &Path) -> Option<(u64, u64)> {
    let mut entries = 0_u64;
    let mut bytes = 0_u64;
    for entry in fs::read_dir(directory).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if entry
            .file_type()
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false)
        {
            // builtin_ir 子目录（wjsm-module 的 lower 产物缓存）纳入统计与淘汰。
            if path.file_name().and_then(|name| name.to_str()) == Some("builtin_ir") {
                let (sub_entries, sub_bytes) = cache_dir_stats(&path)?;
                entries = entries.saturating_add(sub_entries);
                bytes = bytes.saturating_add(sub_bytes);
            }
            continue;
        }
        let path = entry.path();
        let extension = path.extension().and_then(|extension| extension.to_str());
        let is_builtin_ir = is_builtin_ir_path(&path);
        let is_cache_file =
            extension == Some("wnat") || (extension == Some("bin") && is_builtin_ir);
        if !is_cache_file {
            continue;
        }
        let metadata = entry.metadata().ok()?;
        entries = entries.saturating_add(1);
        bytes = bytes.saturating_add(metadata.len());
    }
    Some((entries, bytes))
}

/// 写入后节流触发的 LRU 淘汰：每 [`LRU_CHECK_INTERVAL`] 次写入扫描一次目录，
/// 总字节数超过上限（`WJSM_CACHE_MAX_BYTES`，默认 [`DEFAULT_CACHE_MAX_BYTES`]；
/// `0` 禁用）时按 mtime 删除最旧条目，直到低于上限。
///
/// 节流是全局写入计数而非每目录计数：并发 repository（测试多进程）共享目录时
/// 任一进程的写入都会推进计数，扫描频率只升不降，淘汰始终能覆盖所有写入方。
/// 删除用 `remove_file` 单文件操作，与 `store_cache_entry` 的 create_new+rename
/// 原子写入无竞态：已加载进内存的 image 不受影响，删除只影响后续磁盘命中。
fn maybe_evict_lru(directory: &Path) {
    let counter = WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    if counter % LRU_CHECK_INTERVAL != 0 {
        return;
    }
    let Some(max_bytes) = cache_max_bytes() else {
        return;
    };
    evict_oldest(directory, max_bytes);
}

/// 删除目录中最旧的缓存条目（按 mtime 升序），直到总字节数 ≤ `max_bytes`。
/// 递归覆盖 `builtin_ir/*.bin`。删除是幂等的单文件操作，与并发写入无竞态。
fn evict_oldest(directory: &Path, max_bytes: u64) {
    let mut entries = Vec::new();
    collect_cache_entries(directory, &mut entries);
    let mut bytes: u64 = entries.iter().map(|entry| entry.bytes).sum();
    if bytes <= max_bytes {
        return;
    }
    entries.sort_by_key(|entry| (entry.modified, entry.path.clone()));
    for entry in entries {
        if bytes <= max_bytes {
            break;
        }
        if fs::remove_file(&entry.path).is_ok() {
            bytes = bytes.saturating_sub(entry.bytes);
        }
    }
}

/// 上限：`WJSM_CACHE_MAX_BYTES` 解析为 u64；`0` 或缺失/非法时按默认值处理，
/// 返回 `None` 表示禁用自动淘汰。
fn cache_max_bytes() -> Option<u64> {
    parse_cache_max_bytes(std::env::var_os("WJSM_CACHE_MAX_BYTES"))
}

fn parse_cache_max_bytes(value: Option<std::ffi::OsString>) -> Option<u64> {
    match value {
        Some(value) => {
            let parsed = value.to_str().and_then(|text| text.parse::<u64>().ok());
            match parsed {
                Some(0) => None,
                Some(bytes) => Some(bytes),
                None => Some(DEFAULT_CACHE_MAX_BYTES),
            }
        }
        None => Some(DEFAULT_CACHE_MAX_BYTES),
    }
}

/// 递归收集目录下所有缓存条目（顶层 `.wnat` + `builtin_ir/*.bin`），
/// 记录路径、字节数与 mtime 纳秒。
fn collect_cache_entries(directory: &Path, out: &mut Vec<CacheEntry>) {
    let Ok(read_dir) = fs::read_dir(directory) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) == Some("builtin_ir") {
                collect_cache_entries(&path, out);
            }
            continue;
        }
        let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
            continue;
        };
        let is_cache_file =
            extension == "wnat" || (extension == "bin" && is_builtin_ir_path(&path));
        if !is_cache_file {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        out.push(CacheEntry {
            path,
            bytes: metadata.len(),
            modified: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
        });
    }
}

fn is_builtin_ir_path(path: &Path) -> bool {
    path.parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        == Some("builtin_ir")
}

struct CacheEntry {
    path: PathBuf,
    bytes: u64,
    modified: u128,
}

#[cfg(unix)]
fn set_file_create_permissions(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_file_create_permissions(_options: &mut OpenOptions) {}

#[cfg(unix)]
fn set_directory_permissions(directory: &Path) -> Result<(), NativeCacheError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_directory_permissions(_directory: &Path) -> Result<(), NativeCacheError> {
    Ok(())
}

#[cfg(unix)]
fn validate_cache_permissions(file: &File) -> Result<(), NativeCacheError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    // SAFETY: geteuid 不接收指针且无前置条件。
    let owner = unsafe { libc::geteuid() };
    if metadata.uid() != owner || metadata.mode() & 0o077 != 0 {
        return Err(NativeCacheError::Invalid(
            "cache entry permissions are not owner-only".into(),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_cache_permissions(_file: &File) -> Result<(), NativeCacheError> {
    Ok(())
}

#[derive(Debug, Error)]
pub enum NativeCacheError {
    #[error(transparent)]
    Compile(#[from] NativeCompileError),
    #[error(transparent)]
    Image(#[from] ImageLoadError),
    #[error("native cache I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid native cache entry: {0}")]
    Invalid(String),
    #[error("native cache length overflow")]
    LengthOverflow,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cache_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("wjsm-test-cache")
            .join("backend")
            .join(format!("lru-{tag}-{}-{}", std::process::id(), nanos_now()));
        fs::create_dir_all(&dir).expect("cache dir should be created");
        dir
    }

    fn nanos_now() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    }

    fn touch(path: &Path, bytes: usize) {
        fs::write(path, vec![0_u8; bytes]).expect("test cache entry should be written");
    }

    #[test]
    fn evict_oldest_removes_oldest_wnat_first() {
        let dir = temp_cache_dir("oldest");
        let oldest = dir.join("oldest.wnat");
        let middle = dir.join("middle.wnat");
        let newest = dir.join("newest.wnat");
        touch(&oldest, 10);
        std::thread::sleep(std::time::Duration::from_millis(5));
        touch(&middle, 10);
        std::thread::sleep(std::time::Duration::from_millis(5));
        touch(&newest, 10);

        // 上限 25：删掉最旧的 10 字节后剩 20 ≤ 25。
        evict_oldest(&dir, 25);
        assert!(!oldest.exists(), "最旧条目应先被淘汰");
        assert!(middle.exists());
        assert!(newest.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn evict_oldest_includes_builtin_ir_bin() {
        let dir = temp_cache_dir("builtin");
        let builtin_ir = dir.join("builtin_ir");
        fs::create_dir_all(&builtin_ir).expect("builtin_ir dir should be created");
        let wnat = dir.join("a.wnat");
        let bin = builtin_ir.join("a.bin");
        touch(&wnat, 10);
        std::thread::sleep(std::time::Duration::from_millis(5));
        touch(&bin, 10);

        // 上限 10：两个条目共 20 字节，按 mtime 删 wnat 后剩 10。
        evict_oldest(&dir, 10);
        assert!(!wnat.exists(), "builtin_ir 之外的 .wnat 应参与淘汰");
        assert!(bin.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn evict_oldest_keeps_unrelated_files() {
        let dir = temp_cache_dir("keep");
        touch(&dir.join("a.wnat"), 20);
        fs::write(dir.join("keep.txt"), b"keep").expect("unrelated file should be written");

        evict_oldest(&dir, 0);
        assert!(dir.join("keep.txt").exists(), "非缓存文件不应被 LRU 删除");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_dir_stats_counts_wnat_and_builtin_ir() {
        let dir = temp_cache_dir("stats");
        let builtin_ir = dir.join("builtin_ir");
        fs::create_dir_all(&builtin_ir).expect("builtin_ir dir should be created");
        touch(&dir.join("a.wnat"), 4);
        touch(&builtin_ir.join("b.bin"), 6);
        fs::write(dir.join("ignore.txt"), b"x").expect("unrelated file should be written");

        let (entries, bytes) = cache_dir_stats(&dir).expect("stats should be readable");
        assert_eq!(entries, 2, "wnat 与 builtin_ir/*.bin 各计一个");
        assert_eq!(bytes, 10, "统计字节数应覆盖两者");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_max_bytes_parses_override() {
        use std::ffi::OsString;
        assert_eq!(
            parse_cache_max_bytes(None),
            Some(DEFAULT_CACHE_MAX_BYTES),
            "未设置时用默认上限"
        );
        assert_eq!(
            parse_cache_max_bytes(Some(OsString::from("1048576"))),
            Some(1_048_576),
            "合法字节数应生效"
        );
        assert_eq!(
            parse_cache_max_bytes(Some(OsString::from("0"))),
            None,
            "0 表示禁用自动淘汰"
        );
        assert_eq!(
            parse_cache_max_bytes(Some(OsString::from("garbage"))),
            Some(DEFAULT_CACHE_MAX_BYTES),
            "非法值回退默认上限"
        );
    }
}
