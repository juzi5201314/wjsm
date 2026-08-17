//! 同宿主 native executable 的 overlay payload 与 footer。
//!
//! 不依赖 Cranelift。stub 是 rustc 预链的 ELF/PE；本 crate 只负责在其后
//! 追加 zstd 压缩的 payload，并用固定 footer 定位。
//!
//! 非目标（禁止实现）：
//! - 把 guest `.text` 合进 stub `PT_LOAD`
//! - stub 自解压 / UPX
//! - `libwjsm.so` 或任何旁路共享库
//! - 从 stub 拿掉 Cranelift / 从 overlay 拿掉 `.wjsm`

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;

/// footer 固定 64 字节，始终位于文件末尾。
pub const FOOTER_LEN: usize = 64;
pub const FOOTER_MAGIC: &[u8; 8] = b"WJSMEXEC";
pub const PAYLOAD_SCHEMA: u32 = 4;
/// 内层字节的 zstd 压缩级别；级别 3 是速度/体积的默认点。
const ZSTD_LEVEL: i32 = 3;
pub const FORMAT_VERSION: u16 = 1;

const MAX_PAYLOAD_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_OBJECT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_OBJECTS: u32 = 2;
const MAX_FUNCTIONS: u32 = 4_000_000;
const MAX_STRING_BYTES: u32 = 4096;
const MAX_SNAPSHOT_FILES: u32 = 100_000;
const MAX_SNAPSHOT_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// 预编译 native object 的可移植编码，不含 Cranelift 类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedNativeObject {
    pub bytes: Vec<u8>,
    pub frame_bytes: Vec<u32>,
    pub function_count: u32,
    pub ic_slot_count: u32,
    pub feedback_slot_count: u32,
}

/// overlay 正文：portable artifact + 一段或两段预编译 object。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecPayload {
    pub native_abi_hash: [u8; 32],
    pub codegen_hash: [u8; 32],
    pub target: String,
    pub cranelift_version: String,
    pub settings: String,
    /// 打包期读过的源文件与 `--include`，按 logical URL 索引。
    pub files: BTreeMap<String, Vec<u8>>,
    pub artifact: Vec<u8>,
    pub objects: Vec<EncodedNativeObject>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecFooter {
    pub version: u16,
    pub payload_offset: u64,
    pub payload_len: u64,
    pub digest: [u8; 32],
}

#[derive(Debug, Error)]
pub enum ExecFormatError {
    #[error("native executable footer is missing or truncated")]
    MissingFooter,
    #[error("native executable magic mismatch")]
    InvalidMagic,
    #[error("unsupported native executable format version {0}")]
    UnsupportedVersion(u16),
    #[error("native executable payload digest mismatch")]
    DigestMismatch,
    #[error("native executable payload is truncated")]
    Truncated,
    #[error("invalid native executable payload: {0}")]
    Invalid(String),
    #[error("native executable length overflow")]
    LengthOverflow,
    #[error("native executable I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Stub(String),
}

impl ExecPayload {
    pub fn host_target() -> String {
        format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
    }

    pub fn verify_stub_identity(
        &self,
        native_abi_hash: [u8; 32],
        codegen_hash: [u8; 32],
        cranelift_version: &str,
        settings: &str,
    ) -> Result<(), ExecFormatError> {
        if self.native_abi_hash != native_abi_hash {
            return Err(ExecFormatError::Invalid(
                "native ABI hash does not match this stub".into(),
            ));
        }
        if self.codegen_hash != codegen_hash {
            return Err(ExecFormatError::Invalid(
                "native codegen hash does not match this stub".into(),
            ));
        }
        if self.target != Self::host_target() {
            return Err(ExecFormatError::Invalid(format!(
                "native executable target {} does not match host {}",
                self.target,
                Self::host_target()
            )));
        }
        if self.cranelift_version != cranelift_version {
            return Err(ExecFormatError::Invalid(
                "Cranelift version does not match this stub".into(),
            ));
        }
        if self.settings != settings {
            return Err(ExecFormatError::Invalid(
                "native codegen settings do not match this stub".into(),
            ));
        }
        Ok(())
    }
}

/// 当前平台 stub 文件名。
pub fn stub_file_name() -> &'static str {
    if cfg!(windows) {
        "wjsm-exec.exe"
    } else {
        "wjsm-exec"
    }
}

/// 定位预链 stub：`WJSM_EXEC_STUB`，或当前可执行文件同目录 / `deps` 上一级。
pub fn locate_exec_stub() -> Result<PathBuf, ExecFormatError> {
    if let Some(path) = std::env::var_os("WJSM_EXEC_STUB") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(ExecFormatError::Stub(format!(
            "WJSM_EXEC_STUB is not a file: {}",
            path.display()
        )));
    }
    let exe = std::env::current_exe()?;
    for candidate in stub_candidates(&exe) {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(ExecFormatError::Stub(format!(
        "wjsm-exec stub not found next to {} (set WJSM_EXEC_STUB)",
        exe.display()
    )))
}

fn stub_candidates(current_exe: &Path) -> Vec<PathBuf> {
    let name = stub_file_name();
    let mut candidates = Vec::new();
    let Some(dir) = current_exe.parent() else {
        return candidates;
    };
    candidates.push(dir.join(name));
    if dir.file_name().is_some_and(|file| file == "deps")
        && let Some(parent) = dir.parent()
    {
        candidates.push(parent.join(name));
    }
    candidates
}

/// 若 `bytes` 已带合法 footer，返回去掉 overlay 后的 stub 前缀。
pub fn stub_prefix<'a>(bytes: &'a [u8]) -> &'a [u8] {
    match read_footer(bytes) {
        Ok(footer) => bytes
            .get(..usize::try_from(footer.payload_offset).unwrap_or(0))
            .unwrap_or(bytes),
        Err(_) => bytes,
    }
}

pub fn pack(stub: &[u8], payload: &ExecPayload) -> Result<Vec<u8>, ExecFormatError> {
    let stub = stub_prefix(stub);
    let payload_bytes = encode_payload(payload)?;
    let payload_offset = u64::try_from(stub.len()).map_err(|_| ExecFormatError::LengthOverflow)?;
    let payload_len =
        u64::try_from(payload_bytes.len()).map_err(|_| ExecFormatError::LengthOverflow)?;
    if payload_len > MAX_PAYLOAD_BYTES {
        return Err(ExecFormatError::Invalid(
            "payload exceeds byte limit".into(),
        ));
    }
    let mut out = Vec::with_capacity(
        stub.len()
            .saturating_add(payload_bytes.len())
            .saturating_add(FOOTER_LEN),
    );
    out.extend_from_slice(stub);
    out.extend_from_slice(&payload_bytes);
    out.extend_from_slice(&encode_footer(&ExecFooter {
        version: FORMAT_VERSION,
        payload_offset,
        payload_len,
        digest: sha256(&payload_bytes),
    }));
    Ok(out)
}

pub fn unpack(bytes: &[u8]) -> Result<ExecPayload, ExecFormatError> {
    let footer = read_footer(bytes)?;
    let offset =
        usize::try_from(footer.payload_offset).map_err(|_| ExecFormatError::LengthOverflow)?;
    let len = usize::try_from(footer.payload_len).map_err(|_| ExecFormatError::LengthOverflow)?;
    let end = offset
        .checked_add(len)
        .ok_or(ExecFormatError::LengthOverflow)?;
    let payload = bytes.get(offset..end).ok_or(ExecFormatError::Truncated)?;
    if sha256(payload) != footer.digest {
        return Err(ExecFormatError::DigestMismatch);
    }
    decode_payload(payload)
}

/// 只读文件尾 footer 与 overlay payload，不把 stub 再读进内存。
pub fn unpack_from_path(path: &Path) -> Result<ExecPayload, ExecFormatError> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let file_len = file.metadata()?.len();
    if file_len < FOOTER_LEN as u64 {
        return Err(ExecFormatError::MissingFooter);
    }
    file.seek(SeekFrom::End(-(FOOTER_LEN as i64)))?;
    let mut footer_bytes = [0_u8; FOOTER_LEN];
    file.read_exact(&mut footer_bytes)?;
    let footer = read_footer(&footer_bytes)?;
    let expected_end = footer
        .payload_offset
        .checked_add(footer.payload_len)
        .and_then(|end| end.checked_add(FOOTER_LEN as u64))
        .ok_or(ExecFormatError::LengthOverflow)?;
    if expected_end != file_len {
        return Err(ExecFormatError::Truncated);
    }
    let len = usize::try_from(footer.payload_len).map_err(|_| ExecFormatError::LengthOverflow)?;
    file.seek(SeekFrom::Start(footer.payload_offset))?;
    let mut payload = vec![0_u8; len];
    file.read_exact(&mut payload)?;
    if sha256(&payload) != footer.digest {
        return Err(ExecFormatError::DigestMismatch);
    }
    decode_payload(&payload)
}

pub fn read_footer(bytes: &[u8]) -> Result<ExecFooter, ExecFormatError> {
    if bytes.len() < FOOTER_LEN {
        return Err(ExecFormatError::MissingFooter);
    }
    let footer = &bytes[bytes.len() - FOOTER_LEN..];
    if footer[..8] != *FOOTER_MAGIC {
        return Err(ExecFormatError::InvalidMagic);
    }
    let version = u16::from_le_bytes([footer[8], footer[9]]);
    if version != FORMAT_VERSION {
        return Err(ExecFormatError::UnsupportedVersion(version));
    }
    let payload_offset = u64::from_le_bytes(footer[12..20].try_into().expect("offset width"));
    let payload_len = u64::from_le_bytes(footer[20..28].try_into().expect("length width"));
    if payload_len > MAX_PAYLOAD_BYTES {
        return Err(ExecFormatError::Invalid(
            "payload exceeds byte limit".into(),
        ));
    }
    let digest: [u8; 32] = footer[28..60].try_into().expect("digest width");
    Ok(ExecFooter {
        version,
        payload_offset,
        payload_len,
        digest,
    })
}

fn encode_footer(footer: &ExecFooter) -> [u8; FOOTER_LEN] {
    let mut bytes = [0_u8; FOOTER_LEN];
    bytes[..8].copy_from_slice(FOOTER_MAGIC);
    bytes[8..10].copy_from_slice(&footer.version.to_le_bytes());
    bytes[12..20].copy_from_slice(&footer.payload_offset.to_le_bytes());
    bytes[20..28].copy_from_slice(&footer.payload_len.to_le_bytes());
    bytes[28..60].copy_from_slice(&footer.digest);
    bytes
}

fn encode_payload(payload: &ExecPayload) -> Result<Vec<u8>, ExecFormatError> {
    let inner = encode_inner(payload)?;
    let raw_len = u64::try_from(inner.len()).map_err(|_| ExecFormatError::LengthOverflow)?;
    if raw_len == 0 || raw_len > MAX_PAYLOAD_BYTES {
        return Err(ExecFormatError::Invalid(
            "payload exceeds byte limit".into(),
        ));
    }
    let compressed = zstd::encode_all(inner.as_slice(), ZSTD_LEVEL)
        .map_err(|error| ExecFormatError::Invalid(format!("zstd compress failed: {error}")))?;
    let mut bytes = Vec::with_capacity(12 + compressed.len());
    bytes.extend_from_slice(&PAYLOAD_SCHEMA.to_le_bytes());
    bytes.extend_from_slice(&raw_len.to_le_bytes());
    bytes.extend_from_slice(&compressed);
    Ok(bytes)
}

fn encode_inner(payload: &ExecPayload) -> Result<Vec<u8>, ExecFormatError> {
    if payload.objects.is_empty() || payload.objects.len() > MAX_OBJECTS as usize {
        return Err(ExecFormatError::Invalid(
            "native executable must embed 1 or 2 objects".into(),
        ));
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&payload.native_abi_hash);
    bytes.extend_from_slice(&payload.codegen_hash);
    encode_string(&mut bytes, &payload.target)?;
    encode_string(&mut bytes, &payload.cranelift_version)?;
    encode_string(&mut bytes, &payload.settings)?;
    encode_snapshot_files(&mut bytes, &payload.files)?;
    encode_blob(&mut bytes, &payload.artifact, MAX_ARTIFACT_BYTES)?;
    let object_count =
        u32::try_from(payload.objects.len()).map_err(|_| ExecFormatError::LengthOverflow)?;
    bytes.extend_from_slice(&object_count.to_le_bytes());
    for object in &payload.objects {
        encode_object(&mut bytes, object)?;
    }
    Ok(bytes)
}

fn encode_object(bytes: &mut Vec<u8>, object: &EncodedNativeObject) -> Result<(), ExecFormatError> {
    if object.frame_bytes.len() != usize::try_from(object.function_count).unwrap_or(usize::MAX) {
        return Err(ExecFormatError::Invalid(
            "object frame_bytes length does not match function_count".into(),
        ));
    }
    if object.function_count > MAX_FUNCTIONS {
        return Err(ExecFormatError::Invalid(
            "object function count exceeds limit".into(),
        ));
    }
    bytes.extend_from_slice(&object.function_count.to_le_bytes());
    for frame in &object.frame_bytes {
        bytes.extend_from_slice(&frame.to_le_bytes());
    }
    bytes.extend_from_slice(&object.ic_slot_count.to_le_bytes());
    bytes.extend_from_slice(&object.feedback_slot_count.to_le_bytes());
    encode_blob(bytes, &object.bytes, MAX_OBJECT_BYTES)
}

fn decode_payload(bytes: &[u8]) -> Result<ExecPayload, ExecFormatError> {
    let mut decoder = Decoder::new(bytes);
    if decoder.u32()? != PAYLOAD_SCHEMA {
        return Err(ExecFormatError::Invalid("payload schema mismatch".into()));
    }
    let raw_len = decoder.u64()?;
    if raw_len == 0 || raw_len > MAX_PAYLOAD_BYTES {
        return Err(ExecFormatError::Invalid(
            "payload exceeds byte limit".into(),
        ));
    }
    let inner = decompress_zstd(decoder.remaining(), raw_len)?;
    decode_inner(&inner)
}

fn decompress_zstd(compressed: &[u8], raw_len: u64) -> Result<Vec<u8>, ExecFormatError> {
    let raw_len = usize::try_from(raw_len).map_err(|_| ExecFormatError::LengthOverflow)?;
    let inner = zstd::bulk::Decompressor::new()
        .and_then(|mut decompressor| decompressor.decompress(compressed, raw_len))
        .map_err(|error| ExecFormatError::Invalid(format!("zstd decompress failed: {error}")))?;
    if inner.len() != raw_len {
        return Err(ExecFormatError::Invalid(
            "zstd output length mismatch".into(),
        ));
    }
    Ok(inner)
}

fn decode_inner(bytes: &[u8]) -> Result<ExecPayload, ExecFormatError> {
    let mut decoder = Decoder::new(bytes);
    let native_abi_hash = decoder.hash()?;
    let codegen_hash = decoder.hash()?;
    let target = decoder.string()?;
    let cranelift_version = decoder.string()?;
    let settings = decoder.string()?;
    let files = decode_snapshot_files(&mut decoder)?;
    let artifact = decoder.blob(MAX_ARTIFACT_BYTES)?;
    let object_count = decoder.u32()?;
    if object_count == 0 || object_count > MAX_OBJECTS {
        return Err(ExecFormatError::Invalid(
            "native executable must embed 1 or 2 objects".into(),
        ));
    }
    let mut objects = Vec::with_capacity(object_count as usize);
    for _ in 0..object_count {
        objects.push(decode_object(&mut decoder)?);
    }
    decoder.finish()?;
    Ok(ExecPayload {
        native_abi_hash,
        codegen_hash,
        target,
        cranelift_version,
        settings,
        files,
        artifact,
        objects,
    })
}

fn encode_snapshot_files(
    bytes: &mut Vec<u8>,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<(), ExecFormatError> {
    let count = u32::try_from(files.len()).map_err(|_| ExecFormatError::LengthOverflow)?;
    if count > MAX_SNAPSHOT_FILES {
        return Err(ExecFormatError::Invalid(
            "snapshot file count exceeds limit".into(),
        ));
    }
    bytes.extend_from_slice(&count.to_le_bytes());
    for (logical_url, content) in files {
        if logical_url.is_empty() {
            return Err(ExecFormatError::Invalid(
                "snapshot logical URL is empty".into(),
            ));
        }
        encode_string(bytes, logical_url)?;
        encode_blob(bytes, content, MAX_SNAPSHOT_FILE_BYTES)?;
    }
    Ok(())
}

fn decode_snapshot_files(
    decoder: &mut Decoder<'_>,
) -> Result<BTreeMap<String, Vec<u8>>, ExecFormatError> {
    let count = decoder.u32()?;
    if count > MAX_SNAPSHOT_FILES {
        return Err(ExecFormatError::Invalid(
            "snapshot file count exceeds limit".into(),
        ));
    }
    let mut files = BTreeMap::new();
    for _ in 0..count {
        let logical_url = decoder.string()?;
        if logical_url.is_empty() {
            return Err(ExecFormatError::Invalid(
                "snapshot logical URL is empty".into(),
            ));
        }
        let content = decoder.blob(MAX_SNAPSHOT_FILE_BYTES)?;
        if files.insert(logical_url.clone(), content).is_some() {
            return Err(ExecFormatError::Invalid(format!(
                "duplicate snapshot logical URL {logical_url}"
            )));
        }
    }
    Ok(files)
}

fn decode_object(decoder: &mut Decoder<'_>) -> Result<EncodedNativeObject, ExecFormatError> {
    let function_count = decoder.u32()?;
    if function_count > MAX_FUNCTIONS {
        return Err(ExecFormatError::Invalid(
            "object function count exceeds limit".into(),
        ));
    }
    let count = usize::try_from(function_count).map_err(|_| ExecFormatError::LengthOverflow)?;
    let mut frame_bytes = Vec::with_capacity(count);
    for _ in 0..count {
        frame_bytes.push(decoder.u32()?);
    }
    let ic_slot_count = decoder.u32()?;
    let feedback_slot_count = decoder.u32()?;
    let bytes = decoder.blob(MAX_OBJECT_BYTES)?;
    Ok(EncodedNativeObject {
        bytes,
        frame_bytes,
        function_count,
        ic_slot_count,
        feedback_slot_count,
    })
}

fn encode_string(bytes: &mut Vec<u8>, value: &str) -> Result<(), ExecFormatError> {
    let len = u32::try_from(value.len()).map_err(|_| ExecFormatError::LengthOverflow)?;
    if len > MAX_STRING_BYTES {
        return Err(ExecFormatError::Invalid("string exceeds byte limit".into()));
    }
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(value.as_bytes());
    Ok(())
}

fn encode_blob(bytes: &mut Vec<u8>, blob: &[u8], max: u64) -> Result<(), ExecFormatError> {
    let len = u64::try_from(blob.len()).map_err(|_| ExecFormatError::LengthOverflow)?;
    if len > max {
        return Err(ExecFormatError::Invalid("blob exceeds byte limit".into()));
    }
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(blob);
    Ok(())
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

struct Decoder<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], ExecFormatError> {
        let end = self
            .cursor
            .checked_add(len)
            .ok_or(ExecFormatError::LengthOverflow)?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .ok_or(ExecFormatError::Truncated)?;
        self.cursor = end;
        Ok(value)
    }

    fn u32(&mut self) -> Result<u32, ExecFormatError> {
        Ok(u32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| ExecFormatError::Truncated)?,
        ))
    }

    fn u64(&mut self) -> Result<u64, ExecFormatError> {
        Ok(u64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| ExecFormatError::Truncated)?,
        ))
    }

    fn hash(&mut self) -> Result<[u8; 32], ExecFormatError> {
        self.take(32)?
            .try_into()
            .map_err(|_| ExecFormatError::Truncated)
    }

    fn string(&mut self) -> Result<String, ExecFormatError> {
        let len = self.u32()?;
        if len > MAX_STRING_BYTES {
            return Err(ExecFormatError::Invalid("string exceeds byte limit".into()));
        }
        let bytes = self.take(len as usize)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| ExecFormatError::Invalid("invalid UTF-8".into()))
    }

    fn blob(&mut self, max: u64) -> Result<Vec<u8>, ExecFormatError> {
        let len = self.u64()?;
        if len > max {
            return Err(ExecFormatError::Invalid("blob exceeds byte limit".into()));
        }
        let len = usize::try_from(len).map_err(|_| ExecFormatError::LengthOverflow)?;
        Ok(self.take(len)?.to_vec())
    }

    fn remaining(&self) -> &'a [u8] {
        self.bytes.get(self.cursor..).unwrap_or(&[])
    }

    fn finish(self) -> Result<(), ExecFormatError> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(ExecFormatError::Invalid(
                "payload has trailing bytes".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_payload() -> ExecPayload {
        ExecPayload {
            native_abi_hash: [1; 32],
            codegen_hash: [2; 32],
            target: ExecPayload::host_target(),
            cranelift_version: "0.134.3".into(),
            settings: "opt=speed".into(),
            files: BTreeMap::from([("main.js".into(), b"export {};\n".to_vec())]),
            artifact: b"WJSMART artifact".to_vec(),
            objects: vec![EncodedNativeObject {
                bytes: b"object-bytes".to_vec(),
                frame_bytes: vec![64, 128],
                function_count: 2,
                ic_slot_count: 3,
                feedback_slot_count: 4,
            }],
        }
    }

    #[test]
    fn pack_rejects_empty_snapshot_logical_url() {
        let mut payload = sample_payload();
        payload.files.insert(String::new(), b"x".to_vec());
        assert!(matches!(
            pack(b"stub", &payload),
            Err(ExecFormatError::Invalid(message)) if message.contains("logical URL")
        ));
    }

    #[test]
    fn decode_rejects_schema_2() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&2_u32.to_le_bytes());
        assert!(matches!(
            decode_payload(&bytes),
            Err(ExecFormatError::Invalid(message)) if message.contains("schema")
        ));
    }

    #[test]
    fn decode_rejects_schema_3() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&3_u32.to_le_bytes());
        bytes.extend_from_slice(&16_u64.to_le_bytes());
        assert!(matches!(
            decode_payload(&bytes),
            Err(ExecFormatError::Invalid(message)) if message.contains("schema")
        ));
    }

    #[test]
    fn pack_zstd_shrinks_compressible_payload() {
        let mut payload = sample_payload();
        payload.files.insert(
            "repeat.js".into(),
            "export const x = 1;\n".repeat(4096).into_bytes(),
        );
        let packed = pack(b"stub", &payload).expect("pack");
        let footer = read_footer(&packed).expect("footer");
        let inner_len = encode_inner(&payload).expect("inner").len() as u64;
        assert!(
            footer.payload_len < inner_len,
            "compressed payload {} should be smaller than inner {inner_len}",
            footer.payload_len
        );
        assert_eq!(unpack(&packed).expect("unpack"), payload);
    }

    #[test]
    fn decode_rejects_corrupt_zstd() {
        let inner = encode_inner(&sample_payload()).expect("inner");
        let mut compressed = zstd::encode_all(inner.as_slice(), ZSTD_LEVEL).expect("compress");
        compressed[0] ^= 0xff;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&PAYLOAD_SCHEMA.to_le_bytes());
        bytes.extend_from_slice(&(inner.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&compressed);
        assert!(matches!(
            decode_payload(&bytes),
            Err(ExecFormatError::Invalid(message)) if message.contains("zstd")
        ));
    }

    #[test]
    fn pack_unpack_roundtrip_preserves_payload() {
        let packed = pack(b"\x7fELFstub", &sample_payload()).expect("pack");
        assert!(packed.starts_with(b"\x7fELFstub"));
        let unpacked = unpack(&packed).expect("unpack");
        assert_eq!(unpacked, sample_payload());
    }

    #[test]
    fn pack_strips_existing_overlay_from_stub() {
        let first = pack(b"stub-core", &sample_payload()).expect("first pack");
        let second = pack(&first, &sample_payload()).expect("repack");
        assert_eq!(unpack(&second).expect("unpack"), sample_payload());
        assert!(second.starts_with(b"stub-core"));
        assert_eq!(
            second.iter().filter(|byte| **byte == b's').count(),
            first.iter().filter(|byte| **byte == b's').count()
        );
    }

    #[test]
    fn unpack_rejects_corrupt_digest() {
        let mut packed = pack(b"stub", &sample_payload()).expect("pack");
        let footer = read_footer(&packed).expect("footer");
        let payload_start = usize::try_from(footer.payload_offset).expect("offset");
        packed[payload_start] ^= 0xff;
        assert!(matches!(
            unpack(&packed),
            Err(ExecFormatError::DigestMismatch)
        ));
    }

    #[test]
    fn unpack_rejects_truncated_file() {
        assert!(matches!(
            unpack(b"short"),
            Err(ExecFormatError::MissingFooter)
        ));
    }

    #[test]
    fn verify_stub_identity_checks_hashes_and_target() {
        let payload = sample_payload();
        payload
            .verify_stub_identity([1; 32], [2; 32], "0.134.3", "opt=speed")
            .expect("identity should match");
        assert!(
            payload
                .verify_stub_identity([9; 32], [2; 32], "0.134.3", "opt=speed")
                .is_err()
        );
        assert!(
            payload
                .verify_stub_identity([1; 32], [2; 32], "0.134.3", "opt=size")
                .is_err()
        );
    }

    #[test]
    fn unpack_from_path_reads_only_payload() {
        let packed = pack(b"MZ-pe-stub", &sample_payload()).expect("pack");
        let path = std::env::temp_dir().join(format!(
            "wjsm-exec-format-{}-{}.bin",
            std::process::id(),
            "overlay"
        ));
        std::fs::write(&path, &packed).expect("write");
        let unpacked = unpack_from_path(&path).expect("unpack path");
        let _ = std::fs::remove_file(&path);
        assert_eq!(unpacked, sample_payload());
    }

    #[test]
    fn pack_appends_overlay_without_rewriting_pe_header() {
        let mut stub = vec![0_u8; 128];
        stub[0] = b'M';
        stub[1] = b'Z';
        stub[60] = 64;
        let packed = pack(&stub, &sample_payload()).expect("pack");
        assert_eq!(&packed[..stub.len()], stub.as_slice());
        assert_eq!(unpack(&packed).expect("unpack"), sample_payload());
    }

    #[test]
    fn stub_candidates_include_deps_parent() {
        let exe = Path::new("/tmp/target/debug/deps/wjsm_cli-abc");
        let candidates = stub_candidates(exe);
        assert!(
            candidates
                .iter()
                .any(|path| path.ends_with(stub_file_name()))
        );
        assert!(candidates.iter().any(|path| {
            path == Path::new("/tmp/target/debug")
                .join(stub_file_name())
                .as_path()
        }));
    }
}
