use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};

use sha2::{Digest, Sha256};
use thiserror::Error;
use wjsm_artifact_format::PortableArtifact;

use crate::image::{CompiledImage, ImageLoadError};
use crate::{
    CRANELIFT_VERSION, NATIVE_CODEGEN_HASH, NativeCompileError, NativeCompiler, NativeObject,
    NativeSymbolResolver,
};

const CACHE_MAGIC: &[u8; 8] = b"WJSMNAT\0";
const CACHE_SCHEMA: u32 = 1;
const MAX_CACHE_OBJECT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CACHE_FUNCTIONS: u32 = 4_000_000;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NativeCacheKey {
    pub artifact_digest: [u8; 32],
    pub native_abi_hash: [u8; 32],
    pub codegen_hash: [u8; 32],
    pub target: Arc<str>,
    pub cranelift_version: Arc<str>,
    pub settings: Arc<str>,
}

impl NativeCacheKey {
    pub fn new(artifact: &PortableArtifact, compiler: &NativeCompiler) -> Self {
        Self {
            artifact_digest: artifact.digest(),
            native_abi_hash: wjsm_native_abi::native_abi_hash(),
            codegen_hash: NATIVE_CODEGEN_HASH,
            target: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS).into(),
            cranelift_version: CRANELIFT_VERSION.into(),
            settings: compiler.settings_key().into(),
        }
    }

    pub fn digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(CACHE_SCHEMA.to_le_bytes());
        hasher.update(self.artifact_digest);
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

pub struct NativeImageRepository {
    compiler: NativeCompiler,
    cache_dir: Option<PathBuf>,
    state: Mutex<RepositoryState>,
    stats: AtomicCacheStats,
}

#[derive(Default)]
struct RepositoryState {
    images: HashMap<NativeCacheKey, Weak<CompiledImage>>,
    inflight: HashMap<NativeCacheKey, Arc<InflightGate>>,
}

#[derive(Default)]
struct InflightGate {
    done: Mutex<bool>,
    ready: Condvar,
}

impl NativeImageRepository {
    pub fn new(compiler: NativeCompiler, cache_dir: Option<PathBuf>) -> Self {
        Self {
            compiler,
            cache_dir,
            state: Mutex::new(RepositoryState::default()),
            stats: AtomicCacheStats::default(),
        }
    }

    pub fn prepare(
        &self,
        artifact: &PortableArtifact,
        resolver: &dyn NativeSymbolResolver,
    ) -> Result<Arc<CompiledImage>, NativeCacheError> {
        let key = NativeCacheKey::new(artifact, &self.compiler);
        loop {
            let (gate, leader) = {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(image) = state.images.get(&key).and_then(Weak::upgrade) {
                    self.stats.hits.fetch_add(1, Ordering::Relaxed);
                    return Ok(image);
                }
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

            let prepared = self.prepare_leader(&key, artifact, resolver);
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
        artifact: &PortableArtifact,
        resolver: &dyn NativeSymbolResolver,
    ) -> Result<Arc<CompiledImage>, NativeCacheError> {
        if let Some(directory) = &self.cache_dir {
            match load_cache_entry(directory, key) {
                Ok(Some(object)) => match CompiledImage::load(&object, key.image_id(), resolver) {
                    Ok(image) => {
                        self.stats.hits.fetch_add(1, Ordering::Relaxed);
                        return Ok(image);
                    }
                    Err(_) => {
                        self.invalidate(directory, key);
                    }
                },
                Ok(None) => {}
                Err(_) => self.invalidate(directory, key),
            }
        }
        self.stats.misses.fetch_add(1, Ordering::Relaxed);
        let object = self.compiler.compile(artifact)?;
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
    Ok(())
}

fn encode_cache_entry(
    key: &NativeCacheKey,
    object: &NativeObject,
) -> Result<Vec<u8>, NativeCacheError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(CACHE_MAGIC);
    bytes.extend_from_slice(&CACHE_SCHEMA.to_le_bytes());
    bytes.extend_from_slice(&key.artifact_digest);
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
        artifact_digest: decoder.hash()?,
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
        if entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("wnat")
        {
            continue;
        }
        let metadata = entry.metadata().ok()?;
        entries = entries.saturating_add(1);
        bytes = bytes.saturating_add(metadata.len());
    }
    Some((entries, bytes))
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
