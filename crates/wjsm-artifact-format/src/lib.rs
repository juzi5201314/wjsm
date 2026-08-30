mod manifest;
mod verify;
mod wire_v1;

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

pub use manifest::{ArtifactBuildInput, BuildOptions, ManifestModule, ModuleKind, ModuleManifest};
use sha2::{Digest, Sha256};
use thiserror::Error;
use wjsm_ir::{Builtin, Program};

const MAGIC: &[u8; 8] = b"WJSMART\0";
// v5：运行时NaN-box/属性键布局进入ABI域，旧artifact必须重新构建。
// v8：ObjectSpread 指令新增结果槽（2→3 个 value id），旧artifact必须重新构建。
// v9：SetProp/SetElem 指令新增 strict 位，旧artifact必须重新构建。
// v10：Function 新增类构造器元数据（class_ctor_name），旧artifact必须重新构建。
// v11：Function 新增 JS 可见 name/length 元数据（js_name/js_length），并新增
//      FunctionSetName builtin，旧artifact必须重新构建。
// v12：Function 新增 [[SourceText]]（source_text），并新增 FunctionToString
//      builtin，旧artifact必须重新构建。
// v13：移除 OptionalGetProp/OptionalGetElem/OptionalCall 指令（可选链改为
//      链级短路分叉 + 普通 GetProp/GetElem/Call），旧artifact必须重新构建。
// v14：Call/ConstructCall 指令新增源级 callsite 表达式渲染（TypeError
//      文案对齐 Node 的 `<expr> is not a function/constructor`），旧
//      artifact必须重新构建。
// v15：Guard/LoadSlot/StoreSlot/Deopt 与反馈 80B 槽。
// v16：删除 ElemShapeGuard/GetPropGuarded/GetElemGuarded；GetProp/GetElem 闩锁
//      与 GuardElementsKind 模板并进通用指令。
// v17：Function 新增 env_layout_keys；新增 LoadEnvSlot/StoreEnvSlot（显式 env 操作数）。
const FORMAT_VERSION: u16 = 17;
const HEADER_LEN: usize = 92;
const DIRECTORY_ENTRY_LEN: usize = 52;
const CONTENT_HASH_OFFSET: usize = 60;
const CONTENT_HASH_END: usize = CONTENT_HASH_OFFSET + 32;
const SECTION_MANIFEST: u16 = 1;
const SECTION_PROGRAM: u16 = 2;
const SECTION_SOURCE_MAP: u16 = 3;
const SECTION_SOURCE_TEXT: u16 = 4;
const SECTION_REQUIRED_BUILTINS: u16 = 5;
const SECTION_REQUIRED: u16 = 1;

const _: () = assert!(HEADER_LEN <= u16::MAX as usize);

#[derive(Clone, Debug)]
pub struct ArtifactLimits {
    pub max_total_bytes: u64,
    pub max_section_bytes: u64,
    pub max_sections: u32,
    pub max_modules: u32,
    pub max_module_edges: u32,
    pub max_constants: u32,
    pub max_functions: u32,
    pub max_blocks_per_function: u32,
    pub max_instructions_per_block: u32,
    pub max_phi_sources: u32,
    pub max_switch_cases: u32,
    pub max_values_per_list: u32,
    pub max_strings: u32,
    pub max_string_bytes: u32,
    pub max_required_builtins: u32,
}

impl Default for ArtifactLimits {
    fn default() -> Self {
        Self {
            max_total_bytes: 256 * 1024 * 1024,
            max_section_bytes: 192 * 1024 * 1024,
            max_sections: 16,
            max_modules: 1_000_000,
            max_module_edges: 4_000_000,
            max_constants: 16_000_000,
            max_functions: 4_000_000,
            max_blocks_per_function: 1_000_000,
            max_instructions_per_block: 4_000_000,
            max_phi_sources: 1_000_000,
            max_switch_cases: 1_000_000,
            max_values_per_list: 1_000_000,
            max_strings: 4_000_000,
            max_string_bytes: 64 * 1024 * 1024,
            max_required_builtins: 65_536,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SectionMetadata {
    pub id: u16,
    pub offset: u64,
    pub len: u64,
    pub sha256: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactMetadata {
    pub format_version: u16,
    pub semantic_abi_hash: [u8; 32],
    pub flags: u32,
    pub sections: Vec<SectionMetadata>,
    pub required_builtins: Vec<u16>,
}

#[derive(Clone, Debug)]
pub struct PortableArtifact {
    bytes: Arc<[u8]>,
    program: Arc<Program>,
    manifest: Arc<ModuleManifest>,
    source_text: Option<Arc<str>>,
    metadata: ArtifactMetadata,
    digest: [u8; 32],
}

impl PortableArtifact {
    pub fn from_input(input: &ArtifactBuildInput) -> Result<Self, ArtifactFormatError> {
        verify::verify_artifact(&input.program, &input.manifest)?;
        if input.options.include_source_text && input.source_text.is_none() {
            return Err(ArtifactFormatError::MissingSourceText);
        }

        let manifest_bytes = wire_v1::encode_manifest(&input.manifest)?;
        let program_bytes = wire_v1::encode_program(&input.program)?;
        let required_builtins = wire_v1::encode_required_builtins(&input.program);
        let mut sections = vec![
            EncodedSection::required(SECTION_MANIFEST, manifest_bytes),
            EncodedSection::required(SECTION_PROGRAM, program_bytes),
            EncodedSection::required(SECTION_REQUIRED_BUILTINS, required_builtins),
        ];
        if input.options.include_source_map {
            sections.push(EncodedSection::optional(
                SECTION_SOURCE_MAP,
                encode_source_map(&input.program)?,
            ));
        }
        if let Some(source_text) = input
            .options
            .include_source_text
            .then_some(input.source_text.as_deref())
            .flatten()
        {
            sections.push(EncodedSection::optional(
                SECTION_SOURCE_TEXT,
                source_text.as_bytes().to_vec(),
            ));
        }
        sections.sort_by_key(|section| section.id);
        let bytes: Arc<[u8]> = encode_container(&sections)?.into();
        let digest = read_hash(&bytes[CONTENT_HASH_OFFSET..CONTENT_HASH_END]);
        let metadata = metadata_from_encoded_sections(&sections, &bytes)?;
        Ok(Self {
            bytes,
            program: Arc::clone(&input.program),
            manifest: Arc::clone(&input.manifest),
            metadata,
            source_text: input
                .options
                .include_source_text
                .then(|| input.source_text.as_ref().map(Arc::clone))
                .flatten(),
            digest,
        })
    }

    pub fn decode(bytes: Arc<[u8]>, limits: &ArtifactLimits) -> Result<Self, ArtifactFormatError> {
        let decoded = decode_container(&bytes, limits)?;
        let manifest_bytes = decoded.required(SECTION_MANIFEST)?;
        let program_bytes = decoded.required(SECTION_PROGRAM)?;
        let required_bytes = decoded.required(SECTION_REQUIRED_BUILTINS)?;
        let manifest = Arc::new(wire_v1::decode_manifest(manifest_bytes, limits)?);
        let program = Arc::new(wire_v1::decode_program(program_bytes, limits)?);
        let required_builtins = wire_v1::decode_required_builtins(required_bytes, limits)?;
        let source_text = decoded
            .optional(SECTION_SOURCE_TEXT)?
            .map(|bytes| {
                std::str::from_utf8(bytes)
                    .map(Arc::<str>::from)
                    .map_err(|_| ArtifactFormatError::InvalidUtf8)
            })
            .transpose()?;
        verify::verify_artifact(&program, &manifest)?;
        verify_required_builtin_set(&program, &required_builtins)?;
        let DecodedContainer {
            metadata,
            flags,
            digest,
            ..
        } = decoded;
        Ok(Self {
            bytes,
            program,
            manifest,
            metadata: ArtifactMetadata {
                format_version: FORMAT_VERSION,
                semantic_abi_hash: semantic_abi_hash(),
                flags,
                sections: metadata,
                required_builtins,
            },
            source_text,
            digest,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    pub fn manifest(&self) -> &ModuleManifest {
        &self.manifest
    }
    pub fn source_text(&self) -> Option<&str> {
        self.source_text.as_deref()
    }

    pub fn metadata(&self) -> &ArtifactMetadata {
        &self.metadata
    }

    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }
}

/// 把 Program 编成 portable artifact 使用的确定字节，供 native cache key 计算。
pub fn encode_program_bytes(program: &Program) -> Result<Vec<u8>, ArtifactFormatError> {
    wire_v1::encode_program(program)
}

pub fn semantic_abi_hash() -> [u8; 32] {
    static HASH: OnceLock<[u8; 32]> = OnceLock::new();
    *HASH.get_or_init(|| {
        let mut hasher = Sha256::new();
        hasher.update(b"wjsm-semantic-abi-v1\0");
        hasher.update(include_bytes!("wire_v1.rs"));
        let last = Builtin::last_wire_id();
        for id in 0..=last {
            let builtin = Builtin::from_wire_id(id).expect("contiguous builtin IDs");
            hasher.update(id.to_le_bytes());
            hasher.update(builtin.as_str().as_bytes());
            hasher.update([0]);
        }
        hasher.update(include_bytes!("../../wjsm-ir/src/constants.rs"));
        hasher.update(include_bytes!("../../wjsm-ir/src/value.rs"));
        hasher.finalize().into()
    })
}

#[derive(Debug, Error)]
pub enum ArtifactFormatError {
    #[error("artifact is truncated")]
    Truncated,
    #[error("invalid artifact magic")]
    InvalidMagic,
    #[error("unsupported artifact format version {0}")]
    UnsupportedVersion(u16),
    #[error("invalid artifact header length {0}")]
    InvalidHeaderLength(u16),
    #[error("semantic ABI hash mismatch")]
    SemanticAbiMismatch,
    #[error("artifact total length mismatch: header={header}, actual={actual}")]
    TotalLengthMismatch { header: u64, actual: u64 },
    #[error("artifact content hash mismatch")]
    ContentHashMismatch,
    #[error("section {0} hash mismatch")]
    SectionHashMismatch(u16),
    #[error("duplicate section {0}")]
    DuplicateSection(u16),
    #[error("unknown required section {0}")]
    UnknownRequiredSection(u16),
    #[error("missing required section {0}")]
    MissingRequiredSection(u16),
    #[error("non-canonical artifact: {0}")]
    NonCanonical(String),
    #[error("artifact limit exceeded for {kind}: {actual} > {maximum}")]
    LimitExceeded {
        kind: &'static str,
        actual: u64,
        maximum: u64,
    },
    #[error("integer length or offset overflow")]
    LengthOverflow,
    #[error("unknown {0} tag {1}")]
    UnknownTag(&'static str, u64),
    #[error("invalid boolean byte {0}")]
    InvalidBoolean(u8),
    #[error("invalid UTF-8 string")]
    InvalidUtf8,
    #[error("invalid string constant payload: {0}")]
    InvalidStringPayload(&'static str),
    #[error("section has {0} trailing bytes")]
    TrailingBytes(usize),
    #[error("invalid semantic IR: {0}")]
    InvalidIr(String),
    #[error("invalid module manifest: {0}")]
    InvalidManifest(String),
    #[error("source text section requested without source text")]
    MissingSourceText,
    #[error("required builtin section does not match program")]
    RequiredBuiltinsMismatch,
}

struct EncodedSection {
    id: u16,
    flags: u16,
    bytes: Vec<u8>,
}

impl EncodedSection {
    fn required(id: u16, bytes: Vec<u8>) -> Self {
        Self {
            id,
            flags: SECTION_REQUIRED,
            bytes,
        }
    }

    fn optional(id: u16, bytes: Vec<u8>) -> Self {
        Self {
            id,
            flags: 0,
            bytes,
        }
    }
}

fn encode_container(sections: &[EncodedSection]) -> Result<Vec<u8>, ArtifactFormatError> {
    let section_count =
        u32::try_from(sections.len()).map_err(|_| ArtifactFormatError::LengthOverflow)?;
    let directory_len = DIRECTORY_ENTRY_LEN
        .checked_mul(sections.len())
        .ok_or(ArtifactFormatError::LengthOverflow)?;
    let payload_start = HEADER_LEN
        .checked_add(directory_len)
        .ok_or(ArtifactFormatError::LengthOverflow)?;
    let payload_len = sections.iter().try_fold(0usize, |total, section| {
        total
            .checked_add(section.bytes.len())
            .ok_or(ArtifactFormatError::LengthOverflow)
    })?;
    let total_len = payload_start
        .checked_add(payload_len)
        .ok_or(ArtifactFormatError::LengthOverflow)?;
    let mut bytes = Vec::with_capacity(total_len);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(
        &u16::try_from(HEADER_LEN)
            .map_err(|_| ArtifactFormatError::LengthOverflow)?
            .to_le_bytes(),
    );
    bytes.extend_from_slice(&semantic_abi_hash());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&section_count.to_le_bytes());
    bytes.extend_from_slice(
        &u64::try_from(total_len)
            .map_err(|_| ArtifactFormatError::LengthOverflow)?
            .to_le_bytes(),
    );
    bytes.extend_from_slice(&[0; 32]);

    let mut offset = payload_start;
    for section in sections {
        bytes.extend_from_slice(&section.id.to_le_bytes());
        bytes.extend_from_slice(&section.flags.to_le_bytes());
        bytes.extend_from_slice(
            &u64::try_from(offset)
                .map_err(|_| ArtifactFormatError::LengthOverflow)?
                .to_le_bytes(),
        );
        bytes.extend_from_slice(
            &u64::try_from(section.bytes.len())
                .map_err(|_| ArtifactFormatError::LengthOverflow)?
                .to_le_bytes(),
        );
        bytes.extend_from_slice(&sha256(&section.bytes));
        offset = offset
            .checked_add(section.bytes.len())
            .ok_or(ArtifactFormatError::LengthOverflow)?;
    }
    for section in sections {
        bytes.extend_from_slice(&section.bytes);
    }
    let digest = sha256(&bytes);
    bytes[CONTENT_HASH_OFFSET..CONTENT_HASH_END].copy_from_slice(&digest);
    Ok(bytes)
}

struct DecodedContainer<'a> {
    bytes: &'a [u8],
    sections: BTreeMap<u16, SectionMetadata>,
    metadata: Vec<SectionMetadata>,
    flags: u32,
    digest: [u8; 32],
}

impl<'a> DecodedContainer<'a> {
    fn required(&self, id: u16) -> Result<&'a [u8], ArtifactFormatError> {
        let metadata = self
            .sections
            .get(&id)
            .ok_or(ArtifactFormatError::MissingRequiredSection(id))?;
        let start =
            usize::try_from(metadata.offset).map_err(|_| ArtifactFormatError::LengthOverflow)?;
        let len = usize::try_from(metadata.len).map_err(|_| ArtifactFormatError::LengthOverflow)?;
        let end = start
            .checked_add(len)
            .ok_or(ArtifactFormatError::LengthOverflow)?;
        self.bytes
            .get(start..end)
            .ok_or(ArtifactFormatError::Truncated)
    }

    fn optional(&self, id: u16) -> Result<Option<&'a [u8]>, ArtifactFormatError> {
        if self.sections.contains_key(&id) {
            self.required(id).map(Some)
        } else {
            Ok(None)
        }
    }
}

fn decode_container<'a>(
    bytes: &'a [u8],
    limits: &ArtifactLimits,
) -> Result<DecodedContainer<'a>, ArtifactFormatError> {
    if u64::try_from(bytes.len()).map_err(|_| ArtifactFormatError::LengthOverflow)?
        > limits.max_total_bytes
    {
        return Err(ArtifactFormatError::LimitExceeded {
            kind: "total bytes",
            actual: u64::try_from(bytes.len()).map_err(|_| ArtifactFormatError::LengthOverflow)?,
            maximum: limits.max_total_bytes,
        });
    }
    let header = bytes
        .get(..HEADER_LEN)
        .ok_or(ArtifactFormatError::Truncated)?;
    if header.get(..8) != Some(MAGIC) {
        return Err(ArtifactFormatError::InvalidMagic);
    }
    let version = read_u16(header, 8)?;
    if version != FORMAT_VERSION {
        return Err(ArtifactFormatError::UnsupportedVersion(version));
    }
    let header_len = read_u16(header, 10)?;
    if usize::from(header_len) != HEADER_LEN {
        return Err(ArtifactFormatError::InvalidHeaderLength(header_len));
    }
    if header.get(12..44) != Some(semantic_abi_hash().as_slice()) {
        return Err(ArtifactFormatError::SemanticAbiMismatch);
    }
    let flags = read_u32(header, 44)?;
    let section_count = read_u32(header, 48)?;
    if section_count > limits.max_sections {
        return Err(ArtifactFormatError::LimitExceeded {
            kind: "sections",
            actual: u64::from(section_count),
            maximum: u64::from(limits.max_sections),
        });
    }
    let total_len = read_u64(header, 52)?;
    let actual_len = u64::try_from(bytes.len()).map_err(|_| ArtifactFormatError::LengthOverflow)?;
    if total_len != actual_len {
        return Err(ArtifactFormatError::TotalLengthMismatch {
            header: total_len,
            actual: actual_len,
        });
    }
    let digest = read_hash(&header[CONTENT_HASH_OFFSET..CONTENT_HASH_END]);
    let mut canonical = bytes.to_vec();
    canonical[CONTENT_HASH_OFFSET..CONTENT_HASH_END].fill(0);
    if sha256(&canonical) != digest {
        return Err(ArtifactFormatError::ContentHashMismatch);
    }

    let count = usize::try_from(section_count).map_err(|_| ArtifactFormatError::LengthOverflow)?;
    let directory_end = HEADER_LEN
        .checked_add(
            DIRECTORY_ENTRY_LEN
                .checked_mul(count)
                .ok_or(ArtifactFormatError::LengthOverflow)?,
        )
        .ok_or(ArtifactFormatError::LengthOverflow)?;
    let directory = bytes
        .get(HEADER_LEN..directory_end)
        .ok_or(ArtifactFormatError::Truncated)?;
    let mut sections = BTreeMap::new();
    let mut metadata = Vec::with_capacity(count);
    let mut expected_id = None;
    let mut expected_offset =
        u64::try_from(directory_end).map_err(|_| ArtifactFormatError::LengthOverflow)?;
    for entry in directory.as_chunks::<DIRECTORY_ENTRY_LEN>().0 {
        let id = read_u16(entry, 0)?;
        let section_flags = read_u16(entry, 2)?;
        if expected_id.is_some_and(|previous| previous >= id) {
            return Err(ArtifactFormatError::NonCanonical(
                "section IDs are not strictly increasing".into(),
            ));
        }
        expected_id = Some(id);
        if !matches!(
            id,
            SECTION_MANIFEST
                | SECTION_PROGRAM
                | SECTION_SOURCE_MAP
                | SECTION_SOURCE_TEXT
                | SECTION_REQUIRED_BUILTINS
        ) && section_flags & SECTION_REQUIRED != 0
        {
            return Err(ArtifactFormatError::UnknownRequiredSection(id));
        }
        let offset = read_u64(entry, 4)?;
        let len = read_u64(entry, 12)?;
        if len > limits.max_section_bytes {
            return Err(ArtifactFormatError::LimitExceeded {
                kind: "section bytes",
                actual: len,
                maximum: limits.max_section_bytes,
            });
        }
        if offset != expected_offset {
            return Err(ArtifactFormatError::NonCanonical(
                "section payloads contain a gap or overlap".into(),
            ));
        }
        let end = offset
            .checked_add(len)
            .ok_or(ArtifactFormatError::LengthOverflow)?;
        if end > total_len {
            return Err(ArtifactFormatError::Truncated);
        }
        let start_usize =
            usize::try_from(offset).map_err(|_| ArtifactFormatError::LengthOverflow)?;
        let end_usize = usize::try_from(end).map_err(|_| ArtifactFormatError::LengthOverflow)?;
        let payload = bytes
            .get(start_usize..end_usize)
            .ok_or(ArtifactFormatError::Truncated)?;
        let section_hash = read_hash(&entry[20..52]);
        if sha256(payload) != section_hash {
            return Err(ArtifactFormatError::SectionHashMismatch(id));
        }
        let section = SectionMetadata {
            id,
            offset,
            len,
            sha256: section_hash,
        };
        if sections.insert(id, section.clone()).is_some() {
            return Err(ArtifactFormatError::DuplicateSection(id));
        }
        metadata.push(section);
        expected_offset = end;
    }
    if expected_offset != total_len {
        return Err(ArtifactFormatError::NonCanonical(
            "trailing bytes after final section".into(),
        ));
    }
    for id in [SECTION_MANIFEST, SECTION_PROGRAM, SECTION_REQUIRED_BUILTINS] {
        if !sections.contains_key(&id) {
            return Err(ArtifactFormatError::MissingRequiredSection(id));
        }
    }
    Ok(DecodedContainer {
        bytes,
        sections,
        metadata,
        flags,
        digest,
    })
}

fn metadata_from_encoded_sections(
    sections: &[EncodedSection],
    bytes: &[u8],
) -> Result<ArtifactMetadata, ArtifactFormatError> {
    let decoded = decode_container(bytes, &ArtifactLimits::default())?;
    let required = sections
        .iter()
        .find(|section| section.id == SECTION_REQUIRED_BUILTINS)
        .ok_or(ArtifactFormatError::MissingRequiredSection(
            SECTION_REQUIRED_BUILTINS,
        ))?;
    let required_builtins =
        wire_v1::decode_required_builtins(&required.bytes, &ArtifactLimits::default())?;
    Ok(ArtifactMetadata {
        format_version: FORMAT_VERSION,
        semantic_abi_hash: semantic_abi_hash(),
        flags: decoded.flags,
        sections: decoded.metadata,
        required_builtins,
    })
}

fn verify_required_builtin_set(
    program: &Program,
    expected: &[u16],
) -> Result<(), ArtifactFormatError> {
    let encoded = wire_v1::encode_required_builtins(program);
    let actual = wire_v1::decode_required_builtins(&encoded, &ArtifactLimits::default())?;
    if actual == expected {
        Ok(())
    } else {
        Err(ArtifactFormatError::RequiredBuiltinsMismatch)
    }
}

fn encode_source_map(program: &Program) -> Result<Vec<u8>, ArtifactFormatError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(
        &u32::try_from(program.functions().len())
            .map_err(|_| ArtifactFormatError::LengthOverflow)?
            .to_le_bytes(),
    );
    for function in program.functions() {
        match function.source_span() {
            Some(span) => {
                bytes.push(1);
                bytes.extend_from_slice(&span.line.to_le_bytes());
                bytes.extend_from_slice(&span.col.to_le_bytes());
            }
            None => bytes.push(0),
        }
    }
    Ok(bytes)
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ArtifactFormatError> {
    let end = offset
        .checked_add(2)
        .ok_or(ArtifactFormatError::LengthOverflow)?;
    let value: [u8; 2] = bytes
        .get(offset..end)
        .ok_or(ArtifactFormatError::Truncated)?
        .try_into()
        .expect("fixed-width slice");
    Ok(u16::from_le_bytes(value))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ArtifactFormatError> {
    let end = offset
        .checked_add(4)
        .ok_or(ArtifactFormatError::LengthOverflow)?;
    let value: [u8; 4] = bytes
        .get(offset..end)
        .ok_or(ArtifactFormatError::Truncated)?
        .try_into()
        .expect("fixed-width slice");
    Ok(u32::from_le_bytes(value))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ArtifactFormatError> {
    let end = offset
        .checked_add(8)
        .ok_or(ArtifactFormatError::LengthOverflow)?;
    let value: [u8; 8] = bytes
        .get(offset..end)
        .ok_or(ArtifactFormatError::Truncated)?
        .try_into()
        .expect("fixed-width slice");
    Ok(u64::from_le_bytes(value))
}

fn read_hash(bytes: &[u8]) -> [u8; 32] {
    bytes.try_into().expect("SHA-256 digest has fixed width")
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{BasicBlock, BasicBlockId, Constant, Function, Terminator, ValueId};

    fn sample_input() -> ArtifactBuildInput {
        let mut program = Program::new();
        let constant = program.add_constant(Constant::Number(3.0));
        let key = program.add_constant(Constant::String("x".to_string()));
        let mut function = Function::new("main", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(wjsm_ir::Instruction::Const {
            dest: ValueId(0),
            constant,
        });
        block.push_instruction(wjsm_ir::Instruction::NewObject {
            dest: ValueId(1),
            capacity: 1,
        });
        block.push_instruction(wjsm_ir::Instruction::Const {
            dest: ValueId(2),
            constant: key,
        });
        block.push_instruction(wjsm_ir::Instruction::CreateDataProperty {
            dest: ValueId(3),
            object: ValueId(1),
            key: ValueId(2),
            value: ValueId(0),
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(3)),
        });
        function.push_block(block);
        program.push_function(function);
        ArtifactBuildInput::new(
            program,
            ModuleManifest::single("input.js", true),
            BuildOptions::default(),
        )
    }

    #[test]
    fn deterministic_round_trip() {
        let input = sample_input();
        let first = PortableArtifact::from_input(&input).expect("artifact should encode");
        let second = PortableArtifact::from_input(&input).expect("artifact should encode");
        assert_eq!(first.bytes(), second.bytes());
        let decoded = PortableArtifact::decode(first.bytes.clone(), &ArtifactLimits::default())
            .expect("artifact should decode");
        assert_eq!(decoded.program(), input.program.as_ref());
        assert_eq!(decoded.manifest(), input.manifest.as_ref());
        assert_eq!(decoded.digest(), first.digest());
    }

    #[test]
    fn object_template_name_ref_round_trip() {
        let mut program = Program::new();
        let key_text = program.add_constant(Constant::String("firstName".to_string()));
        let template = program.add_constant(Constant::ObjectTemplate {
            keys: vec![wjsm_ir::value::template_name_ref_key(key_text.0)],
        });
        let input = ArtifactBuildInput::new(
            program,
            ModuleManifest::single("input.js", true),
            BuildOptions::default(),
        );
        let artifact = PortableArtifact::from_input(&input).expect("artifact should encode");
        let decoded = PortableArtifact::decode(artifact.bytes.clone(), &ArtifactLimits::default())
            .expect("artifact should decode");
        let Constant::ObjectTemplate { keys } = &decoded.program().constants()[template.0 as usize]
        else {
            panic!("expected object template constant");
        };
        assert_eq!(keys.len(), 1);
        assert_eq!(
            wjsm_ir::value::template_key_name_ref(keys[0]),
            Some(key_text.0)
        );
    }

    #[test]
    fn utf16_string_constant_round_trip() {
        // 孤立代理项常量（tag 13）：码元序列与烘焙元数据经 wire 往返不变。
        let units = vec![0xD800_u16, 0x0078, 0xDFFF];
        let mut program = Program::new();
        let constant = program.add_constant(Constant::Utf16String(units.clone()));
        let baked = program
            .string_constant_meta(constant)
            .expect("Utf16String 槽位应有烘焙元数据")
            .clone();
        let input = ArtifactBuildInput::new(
            program,
            ModuleManifest::single("input.js", true),
            BuildOptions::default(),
        );
        let artifact = PortableArtifact::from_input(&input).expect("artifact should encode");
        let decoded = PortableArtifact::decode(artifact.bytes.clone(), &ArtifactLimits::default())
            .expect("artifact should decode");
        assert_eq!(
            decoded.program().constants()[constant.0 as usize],
            Constant::Utf16String(units)
        );
        assert_eq!(
            decoded.program().string_constant_meta(constant),
            Some(&baked)
        );
    }

    #[test]
    fn legacy_inline_object_template_still_decodes() {
        let mut program = Program::new();
        let encoded = wjsm_ir::value::encode_inline_ascii(b"name").expect("sso");
        let inline_raw = wjsm_ir::value::inline_property_key_raw(encoded).expect("inline raw");
        program.add_constant(Constant::ObjectTemplate {
            keys: vec![inline_raw],
        });
        let input = ArtifactBuildInput::new(
            program,
            ModuleManifest::single("input.js", true),
            BuildOptions::default(),
        );
        PortableArtifact::from_input(&input).expect("legacy inline template should encode");
    }

    #[test]
    fn source_file_does_not_replace_manifest_logical_url_during_verification() {
        let mut input = sample_input();
        Arc::make_mut(&mut input.program).set_source_file("/home/example/fixtures/input.js");
        PortableArtifact::from_input(&input)
            .expect("independent source and logical paths are valid");
    }

    #[test]
    fn rejects_truncation_and_hash_corruption() {
        let artifact =
            PortableArtifact::from_input(&sample_input()).expect("artifact should encode");
        let truncated: Arc<[u8]> = artifact.bytes()[..artifact.bytes().len() - 1].into();
        assert!(matches!(
            PortableArtifact::decode(truncated, &ArtifactLimits::default()),
            Err(ArtifactFormatError::TotalLengthMismatch { .. })
        ));
        let mut corrupted = artifact.bytes().to_vec();
        let last = corrupted.last_mut().expect("artifact is non-empty");
        *last ^= 1;
        assert!(matches!(
            PortableArtifact::decode(corrupted.into(), &ArtifactLimits::default()),
            Err(ArtifactFormatError::ContentHashMismatch)
        ));
    }
}
