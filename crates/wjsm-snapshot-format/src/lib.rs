//! Startup snapshot binary format: encode/decode + ABI hash.
//!
//! The snapshot is a self-describing little-endian binary with header + sections.
//! The format is designed so that the hot path can bounds-check + slice-copy
//! directly without heap allocations or JSON parsing.

use anyhow::{Result, bail};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

use wjsm_ir::constants;
use wjsm_ir::value;

pub const SNAPSHOT_MAGIC: [u8; 8] = *b"WJSMSNP\0";
/// 格式版本:v8 header 显式记录 immortal objects 相对末尾。
/// 任何 wire 改动必须递增。
pub const SNAPSHOT_FORMAT_VERSION: u32 = 8;

/// `handle_rel_offsets[i]` 的 null 槽哨兵：表示 `obj_table[i] == 0`。
/// 选 `u32::MAX` 因实际 heap 偏移远小于它（heap_used 受 wasm32 线性内存限制），
/// 不会与合法 rel 值碰撞，并显式区分「rel == 0（heap 起点）」与「null 句柄」。
pub const NULL_HANDLE_REL: u32 = u32::MAX;

const HEADER_LEN: usize = 104;

// ── snapshot data types ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct StartupSnapshotHeader {
    pub magic: [u8; 8],
    pub format_version: u32,
    pub abi_hash: u64,
    pub heap_used: u32,
    pub obj_table_count: u32,
    pub function_props_base: u32,
    pub immortal_objects_end_rel: u32,
    pub object_proto_handle: u32,
    pub array_proto_handle: u32,
    pub arr_proto_table_base: u32,
    pub arr_proto_table_len: u32,
    pub arr_proto_table_hash: u64,
    pub iterator_prototype: i64,
    pub generator_prototype: i64,
    pub async_iterator_prototype: i64,
    pub async_gen_prototype: i64,
    pub array_proto_values: i64,
}

pub type SnapshotRuntimeString = Vec<u16>;

/// Owned snapshot suitable for capture/write to disk.
#[derive(Debug, Clone)]
pub struct StartupSnapshotOwned {
    pub header: StartupSnapshotHeader,
    pub object_bytes: Vec<u8>,
    pub handle_rel_offsets: Vec<u32>,
    pub runtime_strings: Vec<SnapshotRuntimeString>,
    pub native_callables: Vec<SnapshotNativeCallable>,
    pub native_callable_methods: Vec<u8>,
}

/// Decoded snapshot view: `object_bytes` 安全借用输入 bytes，
/// 其余字段 owned 以避免 `unsafe`/`leak`。
#[derive(Debug, Clone)]
pub struct StartupSnapshotView<'a> {
    pub header: StartupSnapshotHeader,
    pub object_bytes: &'a [u8],
    pub handle_rel_offsets: Vec<u32>,
    pub runtime_strings: Vec<SnapshotRuntimeString>,
    pub native_callables: Vec<SnapshotNativeCallable>,
    pub native_callable_methods: Vec<u8>,
}

// ── SnapshotNativeCallable ─────────────────────────────────────────

/// Stateless primordial NativeCallable 快照子集。
/// 禁止捕获含运行态 handle/Arc/Mutex 的变体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SnapshotNativeCallable {
    EvalIndirect = 0,
    AsyncIteratorProtoSymbolAsyncIterator = 1,
    ArrayProtoValues = 2,
    ArrayConstructor = 3,
    ObjectConstructor = 4,
    ObjectProtoToString = 5,
    ObjectProtoValueOf = 6,
    FunctionConstructor = 7,
    StringConstructor = 8,
    BooleanConstructor = 9,
    NumberConstructor = 10,
    SymbolConstructor = 11,
    BigIntConstructor = 12,
    RegExpConstructor = 13,
    ErrorConstructor = 14,
    TypeErrorConstructor = 15,
    RangeErrorConstructor = 16,
    SyntaxErrorConstructor = 17,
    ReferenceErrorConstructor = 18,
    URIErrorConstructor = 19,
    EvalErrorConstructor = 20,
    AggregateErrorConstructor = 21,
    MapConstructor = 22,
    SetConstructor = 23,
    WeakMapConstructor = 24,
    WeakSetConstructor = 25,
    WeakRefConstructor = 26,
    FinalizationRegistryConstructor = 27,
    DateConstructorGlobal = 28,
    PromiseConstructor = 29,
    ArrayBufferConstructorGlobal = 30,
    DataViewConstructorGlobal = 31,
    BigInt64ArrayConstructor = 32,
    BigUint64ArrayConstructor = 33,
    ProxyConstructor = 34,
    GcCollect = 35,
    SharedArrayBufferConstructor = 36,
    AtomicsGlobal = 37,
    AgentStart = 38,
    AgentBroadcast = 39,
    AgentReceiveBroadcast = 40,
    AgentGetReport = 41,
    AgentReport = 42,
    AgentSleep = 43,
    AgentMonotonicNow = 44,
    HeadersConstructor = 45,
    ResponseConstructor = 46,
    RequestConstructor = 47,
    AbortControllerConstructor = 48,
    ReadableStreamConstructor = 49,
    WritableStreamConstructor = 50,
    TransformStreamConstructor = 51,
    CountQueuingStrategyConstructor = 52,
    ByteLengthQueuingStrategyConstructor = 53,
    // 预留: 不允许新增运行时状态捕获
    // 最后几项是运行时杂类（不是 constructor）。
    StubGlobal = 54,
    NumberPrimitiveMethod = 55,
    ArgumentsStrictCalleeGetter = 56,
    TypedArrayConstructor = 57,
    BigIntPrimitiveMethod = 58,
    ErrorProtoToString = 59,
    SymbolPrimitiveMethod = 60,
    SymbolProtoDescriptionGetter = 61,
    SymbolProtoToPrimitive = 62,
    RegExpPrimitiveMethod = 63,
    ArrayProtoKeys = 64,
    ArrayProtoEntries = 65,
    IteratorProtoSymbolIterator = 66,
    BufferConstructor = 67,
    TextEncoderConstructor = 68,
    TextDecoderConstructor = 69,
    StructuredClone = 70,
    Atob = 71,
    Btoa = 72,
    QueueMicrotask = 73,
    PerformanceNow = 74,
    OsInfo = 75,
}

impl SnapshotNativeCallable {
    fn from_discriminant(d: u32) -> Option<Self> {
        match d {
            0 => Some(Self::EvalIndirect),
            1 => Some(Self::AsyncIteratorProtoSymbolAsyncIterator),
            2 => Some(Self::ArrayProtoValues),
            3 => Some(Self::ArrayConstructor),
            4 => Some(Self::ObjectConstructor),
            5 => Some(Self::ObjectProtoToString),
            6 => Some(Self::ObjectProtoValueOf),
            7 => Some(Self::FunctionConstructor),
            8 => Some(Self::StringConstructor),
            9 => Some(Self::BooleanConstructor),
            10 => Some(Self::NumberConstructor),
            11 => Some(Self::SymbolConstructor),
            12 => Some(Self::BigIntConstructor),
            13 => Some(Self::RegExpConstructor),
            14 => Some(Self::ErrorConstructor),
            15 => Some(Self::TypeErrorConstructor),
            16 => Some(Self::RangeErrorConstructor),
            17 => Some(Self::SyntaxErrorConstructor),
            18 => Some(Self::ReferenceErrorConstructor),
            19 => Some(Self::URIErrorConstructor),
            20 => Some(Self::EvalErrorConstructor),
            21 => Some(Self::AggregateErrorConstructor),
            22 => Some(Self::MapConstructor),
            23 => Some(Self::SetConstructor),
            24 => Some(Self::WeakMapConstructor),
            25 => Some(Self::WeakSetConstructor),
            26 => Some(Self::WeakRefConstructor),
            27 => Some(Self::FinalizationRegistryConstructor),
            28 => Some(Self::DateConstructorGlobal),
            29 => Some(Self::PromiseConstructor),
            30 => Some(Self::ArrayBufferConstructorGlobal),
            31 => Some(Self::DataViewConstructorGlobal),
            32 => Some(Self::BigInt64ArrayConstructor),
            33 => Some(Self::BigUint64ArrayConstructor),
            34 => Some(Self::ProxyConstructor),
            35 => Some(Self::GcCollect),
            36 => Some(Self::SharedArrayBufferConstructor),
            37 => Some(Self::AtomicsGlobal),
            38 => Some(Self::AgentStart),
            39 => Some(Self::AgentBroadcast),
            40 => Some(Self::AgentReceiveBroadcast),
            41 => Some(Self::AgentGetReport),
            42 => Some(Self::AgentReport),
            43 => Some(Self::AgentSleep),
            44 => Some(Self::AgentMonotonicNow),
            45 => Some(Self::HeadersConstructor),
            46 => Some(Self::ResponseConstructor),
            47 => Some(Self::RequestConstructor),
            48 => Some(Self::AbortControllerConstructor),
            49 => Some(Self::ReadableStreamConstructor),
            50 => Some(Self::WritableStreamConstructor),
            51 => Some(Self::TransformStreamConstructor),
            52 => Some(Self::CountQueuingStrategyConstructor),
            53 => Some(Self::ByteLengthQueuingStrategyConstructor),
            54 => Some(Self::StubGlobal),
            55 => Some(Self::NumberPrimitiveMethod),
            56 => Some(Self::ArgumentsStrictCalleeGetter),
            57 => Some(Self::TypedArrayConstructor),
            58 => Some(Self::BigIntPrimitiveMethod),
            59 => Some(Self::ErrorProtoToString),
            60 => Some(Self::SymbolPrimitiveMethod),
            61 => Some(Self::SymbolProtoDescriptionGetter),
            62 => Some(Self::SymbolProtoToPrimitive),
            63 => Some(Self::RegExpPrimitiveMethod),
            64 => Some(Self::ArrayProtoKeys),
            65 => Some(Self::ArrayProtoEntries),
            66 => Some(Self::IteratorProtoSymbolIterator),
            67 => Some(Self::BufferConstructor),
            68 => Some(Self::TextEncoderConstructor),
            69 => Some(Self::TextDecoderConstructor),
            70 => Some(Self::StructuredClone),
            71 => Some(Self::Atob),
            72 => Some(Self::Btoa),
            73 => Some(Self::QueueMicrotask),
            75 => Some(Self::OsInfo),
            74 => Some(Self::PerformanceNow),
            _ => None,
        }
    }
}

// ── encode ─────────────────────────────────────────────────────────

const SK_OBJECT_BYTES: u32 = 1;
const SK_HANDLE_OFFSETS: u32 = 2;
const SK_RUNTIME_STRINGS: u32 = 3;
const SK_NATIVE_CALLABLES: u32 = 4;
const SECTION_COUNT: u32 = 4;

pub fn encode_snapshot(snapshot: &StartupSnapshotOwned) -> Vec<u8> {
    // 两次写入: 先算 offset，再写最终 buf。
    let header_bytes = build_header_bytes(&snapshot.header);

    // section payloads (pre-serialized)
    let (obj_payload, ho_payload, rs_payload, nc_payload) = build_section_payloads(snapshot);

    // compute offsets
    let header_size = header_bytes.len() as u32;
    let st_start = align_up(header_size, 4);
    let st_size = SECTION_COUNT * 12; // 48
    let payload_start = align_up(st_start + st_size, 4);

    let mut off = payload_start;
    let obj_start = off;
    off += align_up(obj_payload.len() as u32, 4);
    let ho_start = off;
    off += align_up(ho_payload.len() as u32, 4);
    let rs_start = off;
    off += align_up(rs_payload.len() as u32, 4);
    let nc_start = off;

    let total_size = align_up(nc_start + nc_payload.len() as u32, 4) as usize;
    let mut buf = Vec::with_capacity(total_size);

    buf.extend_from_slice(&header_bytes);
    while (buf.len() as u32) < st_start {
        buf.push(0);
    }

    // section table
    write_section_entry(
        &mut buf,
        SK_OBJECT_BYTES,
        obj_start,
        obj_payload.len() as u32,
    );
    write_section_entry(
        &mut buf,
        SK_HANDLE_OFFSETS,
        ho_start,
        ho_payload.len() as u32,
    );
    write_section_entry(
        &mut buf,
        SK_RUNTIME_STRINGS,
        rs_start,
        rs_payload.len() as u32,
    );
    write_section_entry(
        &mut buf,
        SK_NATIVE_CALLABLES,
        nc_start,
        nc_payload.len() as u32,
    );

    while (buf.len() as u32) < payload_start {
        buf.push(0);
    }

    // payload
    append_padded(&mut buf, &obj_payload);
    append_padded(&mut buf, &ho_payload);
    append_padded(&mut buf, &rs_payload);
    buf.extend_from_slice(&nc_payload);

    buf
}

fn build_header_bytes(header: &StartupSnapshotHeader) -> Vec<u8> {
    let mut b = Vec::with_capacity(HEADER_LEN);
    b.extend_from_slice(&header.magic);
    b.extend_from_slice(&header.format_version.to_le_bytes());
    b.extend_from_slice(&header.abi_hash.to_le_bytes());
    b.extend_from_slice(&header.heap_used.to_le_bytes());
    b.extend_from_slice(&header.obj_table_count.to_le_bytes());
    b.extend_from_slice(&header.function_props_base.to_le_bytes());
    b.extend_from_slice(&header.object_proto_handle.to_le_bytes());
    b.extend_from_slice(&header.array_proto_handle.to_le_bytes());
    b.extend_from_slice(&header.arr_proto_table_base.to_le_bytes());
    b.extend_from_slice(&header.arr_proto_table_len.to_le_bytes());
    b.extend_from_slice(&header.arr_proto_table_hash.to_le_bytes());
    b.extend_from_slice(&header.iterator_prototype.to_le_bytes());
    b.extend_from_slice(&header.generator_prototype.to_le_bytes());
    b.extend_from_slice(&header.async_iterator_prototype.to_le_bytes());
    b.extend_from_slice(&header.async_gen_prototype.to_le_bytes());
    b.extend_from_slice(&header.array_proto_values.to_le_bytes());
    b.extend_from_slice(&header.immortal_objects_end_rel.to_le_bytes());
    b.extend_from_slice(&SECTION_COUNT.to_le_bytes());
    b
}

fn build_section_payloads(snapshot: &StartupSnapshotOwned) -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
    let obj_payload = snapshot.object_bytes.clone();

    let mut ho_payload = Vec::with_capacity(snapshot.handle_rel_offsets.len() * 4);
    for off in &snapshot.handle_rel_offsets {
        ho_payload.extend_from_slice(&off.to_le_bytes());
    }

    let mut rs_payload = Vec::new();
    rs_payload.extend_from_slice(&(snapshot.runtime_strings.len() as u32).to_le_bytes());
    for s in &snapshot.runtime_strings {
        rs_payload.extend_from_slice(&(s.len() as u32).to_le_bytes());
        for unit in s {
            rs_payload.extend_from_slice(&unit.to_le_bytes());
        }
    }

    let mut nc_payload = Vec::new();
    nc_payload.extend_from_slice(&(snapshot.native_callables.len() as u32).to_le_bytes());
    for (i, nc) in snapshot.native_callables.iter().enumerate() {
        let method = snapshot
            .native_callable_methods
            .get(i)
            .copied()
            .unwrap_or(0);
        let raw: u32 = (*nc as u32) | ((method as u32) << 8);
        nc_payload.extend_from_slice(&raw.to_le_bytes());
    }

    (obj_payload, ho_payload, rs_payload, nc_payload)
}

fn write_section_entry(buf: &mut Vec<u8>, kind: u32, offset: u32, len: u32) {
    buf.extend_from_slice(&kind.to_le_bytes());
    buf.extend_from_slice(&offset.to_le_bytes());
    buf.extend_from_slice(&len.to_le_bytes());
}

fn append_padded(buf: &mut Vec<u8>, data: &[u8]) {
    buf.extend_from_slice(data);
    while !buf.len().is_multiple_of(4) {
        buf.push(0);
    }
}

// ── decode ─────────────────────────────────────────────────────────

/// Decode a snapshot from bytes. `object_bytes` 安全借用输入 bytes，
/// 其余字段 owned；不再使用 `unsafe`/`leak`/`from_raw_parts`。
/// `runtime_strings` 以 UTF-16 code units 返回，调用方直接用于 `RuntimeState`。
pub fn decode_snapshot(bytes: &[u8]) -> Result<StartupSnapshotView<'_>> {
    if bytes.len() < HEADER_LEN {
        bail!("snapshot too short: {} bytes", bytes.len());
    }

    let magic: [u8; 8] = bytes[0..8].try_into()?;
    if magic != SNAPSHOT_MAGIC {
        bail!("bad snapshot magic: {:02x?}", magic);
    }

    let format_version = u32::from_le_bytes(bytes[8..12].try_into()?);
    if format_version != SNAPSHOT_FORMAT_VERSION {
        bail!(
            "unsupported snapshot format version: {} (expected {})",
            format_version,
            SNAPSHOT_FORMAT_VERSION
        );
    }

    let abi_hash = u64::from_le_bytes(bytes[12..20].try_into()?);
    let heap_used = u32::from_le_bytes(bytes[20..24].try_into()?);
    let obj_table_count = u32::from_le_bytes(bytes[24..28].try_into()?);
    let function_props_base = u32::from_le_bytes(bytes[28..32].try_into()?);
    let object_proto_handle = u32::from_le_bytes(bytes[32..36].try_into()?);
    let array_proto_handle = u32::from_le_bytes(bytes[36..40].try_into()?);
    let arr_proto_table_base = u32::from_le_bytes(bytes[40..44].try_into()?);
    let arr_proto_table_len = u32::from_le_bytes(bytes[44..48].try_into()?);
    let arr_proto_table_hash = u64::from_le_bytes(bytes[48..56].try_into()?);
    let iterator_prototype = i64::from_le_bytes(bytes[56..64].try_into()?);
    let generator_prototype = i64::from_le_bytes(bytes[64..72].try_into()?);
    let async_iterator_prototype = i64::from_le_bytes(bytes[72..80].try_into()?);
    let async_gen_prototype = i64::from_le_bytes(bytes[80..88].try_into()?);
    let array_proto_values = i64::from_le_bytes(bytes[88..96].try_into()?);
    let immortal_objects_end_rel = u32::from_le_bytes(bytes[96..100].try_into()?);

    let section_count = u32::from_le_bytes(bytes[100..104].try_into()?) as usize;
    if section_count > 16 {
        bail!("too many sections: {}", section_count);
    }

    let header = StartupSnapshotHeader {
        magic,
        format_version,
        abi_hash,
        heap_used,
        immortal_objects_end_rel,
        obj_table_count,
        function_props_base,
        object_proto_handle,
        array_proto_handle,
        arr_proto_table_base,
        arr_proto_table_len,
        arr_proto_table_hash,
        iterator_prototype,
        generator_prototype,
        async_iterator_prototype,
        async_gen_prototype,
        array_proto_values,
    };

    // section table starts after the fixed-size header.
    let st_start = HEADER_LEN;
    if bytes.len() < st_start + section_count * 12 {
        bail!("section table truncated");
    }

    let mut object_bytes: &[u8] = &[];
    let mut handle_rel_offsets: Vec<u32> = Vec::new();
    let mut runtime_strings: Vec<SnapshotRuntimeString> = Vec::new();
    let mut native_callables: Vec<SnapshotNativeCallable> = Vec::new();
    let mut native_callable_methods: Vec<u8> = Vec::new();
    let mut seen_object = false;
    let mut seen_handles = false;
    let mut seen_strings = false;
    let mut seen_native = false;

    for i in 0..section_count {
        let off = st_start + i * 12;
        let _kind = u32::from_le_bytes(bytes[off..off + 4].try_into()?);
        let sect_off = u32::from_le_bytes(bytes[off + 4..off + 8].try_into()?) as usize;
        let sect_len = u32::from_le_bytes(bytes[off + 8..off + 12].try_into()?) as usize;
        if sect_off
            .checked_add(sect_len)
            .is_none_or(|e| e > bytes.len())
        {
            bail!(
                "section {} offset={} len={} out of bounds",
                i,
                sect_off,
                sect_len
            );
        }
        let data = &bytes[sect_off..sect_off + sect_len];

        match _kind {
            SK_OBJECT_BYTES => {
                if seen_object {
                    bail!("duplicate section kind {}", _kind);
                }
                seen_object = true;
                object_bytes = data;
            }
            SK_HANDLE_OFFSETS => {
                if seen_handles {
                    bail!("duplicate section kind {}", _kind);
                }
                seen_handles = true;
                if !data.len().is_multiple_of(4) {
                    bail!(
                        "handle_rel_offsets data length {} is not a multiple of 4",
                        data.len()
                    );
                }
                handle_rel_offsets = data
                    .chunks_exact(4)
                    .map(|c| u32::from_le_bytes(c.try_into().unwrap_or([0; 4])))
                    .collect();
                if handle_rel_offsets.len() != header.obj_table_count as usize {
                    bail!(
                        "handle_rel_offsets count {} != header.obj_table_count {}",
                        handle_rel_offsets.len(),
                        header.obj_table_count
                    );
                }
            }
            SK_RUNTIME_STRINGS => {
                if seen_strings {
                    bail!("duplicate section kind {}", _kind);
                }
                seen_strings = true;
                if data.len() < 4 {
                    bail!("runtime_strings section too short");
                }
                let count = u32::from_le_bytes(data[0..4].try_into()?) as usize;
                let mut strings: Vec<SnapshotRuntimeString> = Vec::with_capacity(count);
                let mut pos = 4usize;
                for _ in 0..count {
                    if pos + 4 > data.len() {
                        bail!("runtime_strings entry truncated");
                    }
                    let unit_len = u32::from_le_bytes(data[pos..pos + 4].try_into()?) as usize;
                    pos += 4;
                    let byte_len = unit_len.checked_mul(2).ok_or_else(|| {
                        anyhow::anyhow!("runtime_strings entry byte length overflow")
                    })?;
                    if pos + byte_len > data.len() {
                        bail!("runtime_strings entry body truncated");
                    }
                    let mut units = Vec::with_capacity(unit_len);
                    for chunk in data[pos..pos + byte_len].chunks_exact(2) {
                        units.push(u16::from_le_bytes(chunk.try_into()?));
                    }
                    strings.push(units);
                    pos += byte_len;
                }
                runtime_strings = strings;
            }
            SK_NATIVE_CALLABLES => {
                if seen_native {
                    bail!("duplicate section kind {}", _kind);
                }
                seen_native = true;
                if data.len() < 4 {
                    bail!("native_callables section too short");
                }
                let count = u32::from_le_bytes(data[0..4].try_into()?) as usize;
                if data.len() < 4 + count * 4 {
                    bail!("native_callables section truncated");
                }
                let mut ncs: Vec<SnapshotNativeCallable> = Vec::with_capacity(count);
                let mut methods: Vec<u8> = Vec::with_capacity(count);
                for j in 0..count {
                    let raw = u32::from_le_bytes(data[4 + j * 4..8 + j * 4].try_into()?);
                    let d = raw & 0xFF;
                    let method = (raw >> 8) as u8;
                    let nc = SnapshotNativeCallable::from_discriminant(d).ok_or_else(|| {
                        anyhow::anyhow!("unknown native callable discriminant {}", d)
                    })?;
                    ncs.push(nc);
                    methods.push(method);
                }
                native_callables = ncs;
                native_callable_methods = methods;
            }
            _ => bail!("unknown snapshot section kind {}", _kind),
        }
    }

    if !seen_object || !seen_handles || !seen_strings || !seen_native {
        bail!(
            "missing required snapshot sections (object={}, handles={}, strings={}, native={})",
            seen_object,
            seen_handles,
            seen_strings,
            seen_native
        );
    }
    if object_bytes.len() != heap_used as usize {
        bail!(
            "object_bytes len {} != header.heap_used {}",
            object_bytes.len(),
            heap_used
        );
    }
    if immortal_objects_end_rel > heap_used {
        bail!(
            "header.immortal_objects_end_rel {} > heap_used {}",
            immortal_objects_end_rel,
            heap_used
        );
    }
    for (i, &rel) in handle_rel_offsets.iter().enumerate() {
        if rel != NULL_HANDLE_REL && rel >= heap_used {
            bail!(
                "handle_rel_offsets[{}] rel {} >= heap_used {}",
                i,
                rel,
                heap_used
            );
        }
    }
    Ok(StartupSnapshotView {
        header,
        object_bytes,
        handle_rel_offsets,
        runtime_strings,
        native_callables,
        native_callable_methods,
    })
}

// ── ABI hash ────────────────────────────────────────────────────────

pub fn abi_hash() -> u64 {
    let mut hasher = DefaultHasher::new();
    SNAPSHOT_FORMAT_VERSION.hash(&mut hasher);

    // NaN-box constants
    value::BOX_BASE.hash(&mut hasher);
    value::TAG_MASK.hash(&mut hasher);
    value::TAG_STRING.hash(&mut hasher);
    value::TAG_UNDEFINED.hash(&mut hasher);
    value::TAG_NULL.hash(&mut hasher);
    value::TAG_BOOL.hash(&mut hasher);
    value::TAG_ITERATOR.hash(&mut hasher);
    value::TAG_ENUMERATOR.hash(&mut hasher);
    value::TAG_NATIVE_CALLABLE.hash(&mut hasher);
    value::TAG_OBJECT.hash(&mut hasher);
    value::TAG_FUNCTION.hash(&mut hasher);
    value::TAG_CLOSURE.hash(&mut hasher);
    value::TAG_ARRAY.hash(&mut hasher);
    value::TAG_BOUND.hash(&mut hasher);
    value::TAG_BIGINT.hash(&mut hasher);
    value::TAG_SYMBOL.hash(&mut hasher);
    value::TAG_REGEXP.hash(&mut hasher);
    value::TAG_PROXY.hash(&mut hasher);
    value::TAG_SCOPE_RECORD.hash(&mut hasher);
    value::TAG_ARRAY_HOLE.hash(&mut hasher);

    // Heap type tags
    wjsm_ir::HEAP_TYPE_OBJECT.hash(&mut hasher);
    wjsm_ir::HEAP_TYPE_ARRAY.hash(&mut hasher);
    wjsm_ir::HEAP_TYPE_PROMISE.hash(&mut hasher);
    wjsm_ir::HEAP_TYPE_CONTINUATION.hash(&mut hasher);
    wjsm_ir::HEAP_TYPE_ASYNC_GENERATOR.hash(&mut hasher);
    wjsm_ir::HEAP_TYPE_ARGUMENTS.hash(&mut hasher);

    // Primordial string table
    for (offset, s) in constants::primordial_string_offsets() {
        offset.hash(&mut hasher);
        s.hash(&mut hasher);
    }

    // SnapshotNativeCallable discriminants in order
    for d in 0u32..=75 {
        if let Some(_nc) = SnapshotNativeCallable::from_discriminant(d) {
            // hash the discriminant
            d.hash(&mut hasher);
        }
    }

    // Property slot constants
    constants::PROP_SLOT_SIZE.hash(&mut hasher);
    constants::PROP_SLOT_NAME_ID_OFFSET.hash(&mut hasher);
    constants::PROP_SLOT_FLAGS_OFFSET.hash(&mut hasher);
    constants::PROP_SLOT_VALUE_OFFSET.hash(&mut hasher);
    constants::PROP_SLOT_GETTER_OFFSET.hash(&mut hasher);
    constants::PROP_SLOT_SETTER_OFFSET.hash(&mut hasher);
    constants::FLAG_CONFIGURABLE.hash(&mut hasher);
    constants::FLAG_ENUMERABLE.hash(&mut hasher);
    constants::FLAG_WRITABLE.hash(&mut hasher);
    constants::FLAG_IS_ACCESSOR.hash(&mut hasher);
    constants::FLAG_PRIVATE.hash(&mut hasher);

    // Heap / handle-table layout constants
    for (name, value) in constants::heap_layout_abi_inputs() {
        name.hash(&mut hasher);
        value.hash(&mut hasher);
    }
    wjsm_ir::SHADOW_STACK_SIZE.hash(&mut hasher);
    wjsm_ir::SHADOW_STACK_HEAP_GUARD_SIZE.hash(&mut hasher);
    wjsm_ir::SHADOW_STACK_HEAP_GUARD_CANARY.hash(&mut hasher);
    // Embedded support module / builtin JS bundle hash（运行时通过
    // `register_abi_hash_external_input` 注入；未注入时为 0，不参与 hash 改变）。
    if let Some(extra) = ABI_HASH_EXTERNAL_INPUT.get() {
        extra.hash(&mut hasher);
    }

    hasher.finish()
}

/// 进程级注入：embedded support module / builtin JS bundle 的额外 ABI 输入。
/// 由 `wjsm-runtime` 在启动时 set 一次（运行时输入，初始化时刻 + 来源都不在
/// 本 crate 静态期可知，所以只能用 `OnceLock`，不可换 `LazyLock`）。
/// 重复 set 静默忽略；这是为了让 build.rs 与运行时共享同一注入点。
static ABI_HASH_EXTERNAL_INPUT: OnceLock<u64> = OnceLock::new();

pub fn register_abi_hash_external_input(value: u64) {
    let _ = ABI_HASH_EXTERNAL_INPUT.set(value);
}

fn align_up(n: u32, align: u32) -> u32 {
    (n + align - 1) & !(align - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected_static_abi_hash() -> u64 {
        let mut hasher = DefaultHasher::new();
        SNAPSHOT_FORMAT_VERSION.hash(&mut hasher);

        value::BOX_BASE.hash(&mut hasher);
        value::TAG_MASK.hash(&mut hasher);
        value::TAG_STRING.hash(&mut hasher);
        value::TAG_UNDEFINED.hash(&mut hasher);
        value::TAG_NULL.hash(&mut hasher);
        value::TAG_BOOL.hash(&mut hasher);
        value::TAG_ITERATOR.hash(&mut hasher);
        value::TAG_ENUMERATOR.hash(&mut hasher);
        value::TAG_NATIVE_CALLABLE.hash(&mut hasher);
        value::TAG_OBJECT.hash(&mut hasher);
        value::TAG_FUNCTION.hash(&mut hasher);
        value::TAG_CLOSURE.hash(&mut hasher);
        value::TAG_ARRAY.hash(&mut hasher);
        value::TAG_BOUND.hash(&mut hasher);
        value::TAG_BIGINT.hash(&mut hasher);
        value::TAG_SYMBOL.hash(&mut hasher);
        value::TAG_REGEXP.hash(&mut hasher);
        value::TAG_PROXY.hash(&mut hasher);
        value::TAG_SCOPE_RECORD.hash(&mut hasher);
        value::TAG_ARRAY_HOLE.hash(&mut hasher);

        wjsm_ir::HEAP_TYPE_OBJECT.hash(&mut hasher);
        wjsm_ir::HEAP_TYPE_ARRAY.hash(&mut hasher);
        wjsm_ir::HEAP_TYPE_PROMISE.hash(&mut hasher);
        wjsm_ir::HEAP_TYPE_CONTINUATION.hash(&mut hasher);
        wjsm_ir::HEAP_TYPE_ASYNC_GENERATOR.hash(&mut hasher);
        wjsm_ir::HEAP_TYPE_ARGUMENTS.hash(&mut hasher);

        for (offset, s) in constants::primordial_string_offsets() {
            offset.hash(&mut hasher);
            s.hash(&mut hasher);
        }

        for d in 0u32..=75 {
            if SnapshotNativeCallable::from_discriminant(d).is_some() {
                d.hash(&mut hasher);
            }
        }

        constants::PROP_SLOT_SIZE.hash(&mut hasher);
        constants::PROP_SLOT_NAME_ID_OFFSET.hash(&mut hasher);
        constants::PROP_SLOT_FLAGS_OFFSET.hash(&mut hasher);
        constants::PROP_SLOT_VALUE_OFFSET.hash(&mut hasher);
        constants::PROP_SLOT_GETTER_OFFSET.hash(&mut hasher);
        constants::PROP_SLOT_SETTER_OFFSET.hash(&mut hasher);
        constants::FLAG_CONFIGURABLE.hash(&mut hasher);
        constants::FLAG_ENUMERABLE.hash(&mut hasher);
        constants::FLAG_WRITABLE.hash(&mut hasher);
        constants::FLAG_IS_ACCESSOR.hash(&mut hasher);
        constants::FLAG_PRIVATE.hash(&mut hasher);

        for (name, value) in constants::heap_layout_abi_inputs() {
            name.hash(&mut hasher);
            value.hash(&mut hasher);
        }
        wjsm_ir::SHADOW_STACK_SIZE.hash(&mut hasher);
        wjsm_ir::SHADOW_STACK_HEAP_GUARD_SIZE.hash(&mut hasher);
        wjsm_ir::SHADOW_STACK_HEAP_GUARD_CANARY.hash(&mut hasher);

        hasher.finish()
    }

    fn snapshot_with_runtime_strings(
        runtime_strings: Vec<SnapshotRuntimeString>,
    ) -> StartupSnapshotOwned {
        StartupSnapshotOwned {
            header: StartupSnapshotHeader {
                magic: SNAPSHOT_MAGIC,
                format_version: SNAPSHOT_FORMAT_VERSION,
                abi_hash: abi_hash(),
                heap_used: 0,
                immortal_objects_end_rel: 0,
                obj_table_count: 0,
                function_props_base: 0,
                object_proto_handle: 0,
                array_proto_handle: 0,
                arr_proto_table_base: 0,
                arr_proto_table_len: 0,
                arr_proto_table_hash: 0,
                iterator_prototype: 0,
                generator_prototype: 0,
                async_iterator_prototype: 0,
                async_gen_prototype: 0,
                array_proto_values: 0,
            },
            object_bytes: Vec::new(),
            handle_rel_offsets: Vec::new(),
            runtime_strings,
            native_callables: Vec::new(),
            native_callable_methods: Vec::new(),
        }
    }

    #[test]
    fn snapshot_format_version_is_v8_immortal_boundary() {
        assert_eq!(SNAPSHOT_FORMAT_VERSION, 8);
    }

    #[test]
    fn runtime_string_section_roundtrips_utf16_units() {
        let snapshot = snapshot_with_runtime_strings(vec![vec![0xD800], vec![0x0041, 0xDFFF]]);

        let bytes = encode_snapshot(&snapshot);
        let decoded = decode_snapshot(&bytes).expect("snapshot decodes");

        assert_eq!(
            decoded.runtime_strings,
            vec![vec![0xD800], vec![0x0041, 0xDFFF]]
        );
    }

    #[test]
    fn immortal_objects_end_header_roundtrips() {
        let mut snapshot = snapshot_with_runtime_strings(Vec::new());
        snapshot.header.heap_used = 16;
        snapshot.header.immortal_objects_end_rel = 12;
        snapshot.object_bytes = vec![0; 16];

        let bytes = encode_snapshot(&snapshot);
        let decoded = decode_snapshot(&bytes).expect("snapshot decodes");

        assert_eq!(decoded.header.heap_used, 16);
        assert_eq!(decoded.header.immortal_objects_end_rel, 12);
    }

    #[test]
    fn abi_hash_includes_heap_layout_inputs() {
        assert_eq!(abi_hash(), expected_static_abi_hash());
    }

    #[test]
    fn heap_layout_abi_inputs_cover_snapshot_heap_shape() {
        let inputs = constants::heap_layout_abi_inputs();
        for required in [
            "heap_object_header_size",
            "heap_object_capacity_offset",
            "heap_object_property_count_offset",
            "heap_object_property_slot_size",
            "heap_array_length_offset",
            "heap_array_capacity_offset",
            "heap_array_element_size",
            "handle_table_entry_size",
            "handle_table_min_entries",
            "handle_table_function_entry_factor",
            "heap_allocation_alignment",
            "gc_region_size",
            "gc_card_size",
            "gc_barrier_event_size",
            "gc_barrier_event_buffer_size",
        ] {
            assert!(
                inputs.iter().any(|(name, _)| *name == required),
                "missing heap layout ABI input `{required}`"
            );
        }
    }
}
