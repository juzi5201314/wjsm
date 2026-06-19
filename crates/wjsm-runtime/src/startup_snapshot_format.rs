//! Startup snapshot binary format: encode/decode + ABI hash.
//!
//! The snapshot is a self-describing little-endian binary with header + sections.
//! The format is designed so that the hot path can bounds-check + slice-copy
//! directly without heap allocations or JSON parsing.

use anyhow::{bail, Result};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::types::NativeCallable;
use wjsm_ir::constants;
use wjsm_ir::value;

pub(crate) const SNAPSHOT_MAGIC: [u8; 8] = *b"WJSMSNP\0";
/// 格式版本：obj_table 槽编码方式变化（v1 用 0 区分 null 与 heap_start，v2 用
/// `NULL_HANDLE_REL` 哨兵）。任何 wire 改动必须递增。
pub(crate) const SNAPSHOT_FORMAT_VERSION: u32 = 2;

/// `handle_rel_offsets[i]` 的 null 槽哨兵：表示 `obj_table[i] == 0`。
/// 选 `u32::MAX` 因实际 heap 偏移远小于它（heap_used 受 wasm32 线性内存限制），
/// 不会与合法 rel 值碰撞，并显式区分「rel == 0（heap 起点）」与「null 句柄」。
pub(crate) const NULL_HANDLE_REL: u32 = u32::MAX;

// ── snapshot data types ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct StartupSnapshotHeader {
    pub magic: [u8; 8],
    pub format_version: u32,
    pub abi_hash: u64,
    pub heap_used: u32,
    pub obj_table_count: u32,
    pub function_props_base: u32,
    pub object_proto_handle: u32,
    pub array_proto_handle: u32,
    pub async_iterator_prototype: i64,
    pub async_gen_prototype: i64,
    pub array_proto_values: i64,
}

/// Owned snapshot suitable for capture/write to disk.
#[derive(Debug, Clone)]
pub(crate) struct StartupSnapshotOwned {
    pub header: StartupSnapshotHeader,
    pub object_bytes: Vec<u8>,
    pub handle_rel_offsets: Vec<u32>,
    pub runtime_strings: Vec<String>,
    pub native_callables: Vec<SnapshotNativeCallable>,
}

/// Decoded snapshot view: `object_bytes` 安全借用输入 bytes，
/// 其余字段 owned 以避免 `unsafe`/`leak`。
#[derive(Debug, Clone)]
pub(crate) struct StartupSnapshotView<'a> {
    pub header: StartupSnapshotHeader,
    pub object_bytes: &'a [u8],
    pub handle_rel_offsets: Vec<u32>,
    pub runtime_strings: Vec<String>,
    pub native_callables: Vec<SnapshotNativeCallable>,
}

// ── SnapshotNativeCallable ─────────────────────────────────────────

/// Stateless primordial NativeCallable 快照子集。
/// 禁止捕获含运行态 handle/Arc/Mutex 的变体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum SnapshotNativeCallable {
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
    // 最后 3 个是运行时杂类（不是 constructor）。
    StubGlobal = 54,
    NumberPrimitiveMethod = 55,
    ArgumentsStrictCalleeGetter = 56,
    TypedArrayConstructor = 57,
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
            _ => None,
        }
    }

    pub(crate) fn into_native_callable(self) -> NativeCallable {
        match self {
            Self::EvalIndirect => NativeCallable::EvalIndirect,
            Self::AsyncIteratorProtoSymbolAsyncIterator => {
                NativeCallable::AsyncIteratorProtoSymbolAsyncIterator
            }
            Self::ArrayProtoValues => NativeCallable::ArrayProtoValues,
            Self::ArrayConstructor => NativeCallable::ArrayConstructor,
            Self::ObjectConstructor => NativeCallable::ObjectConstructor,
            Self::ObjectProtoToString => NativeCallable::ObjectProtoToString,
            Self::ObjectProtoValueOf => NativeCallable::ObjectProtoValueOf,
            Self::FunctionConstructor => NativeCallable::FunctionConstructor,
            Self::StringConstructor => NativeCallable::StringConstructor,
            Self::BooleanConstructor => NativeCallable::BooleanConstructor,
            Self::NumberConstructor => NativeCallable::NumberConstructor,
            Self::SymbolConstructor => NativeCallable::SymbolConstructor,
            Self::BigIntConstructor => NativeCallable::BigIntConstructor,
            Self::RegExpConstructor => NativeCallable::RegExpConstructor,
            Self::ErrorConstructor => NativeCallable::ErrorConstructor,
            Self::TypeErrorConstructor => NativeCallable::TypeErrorConstructor,
            Self::RangeErrorConstructor => NativeCallable::RangeErrorConstructor,
            Self::SyntaxErrorConstructor => NativeCallable::SyntaxErrorConstructor,
            Self::ReferenceErrorConstructor => NativeCallable::ReferenceErrorConstructor,
            Self::URIErrorConstructor => NativeCallable::URIErrorConstructor,
            Self::EvalErrorConstructor => NativeCallable::EvalErrorConstructor,
            Self::AggregateErrorConstructor => NativeCallable::AggregateErrorConstructor,
            Self::MapConstructor => NativeCallable::MapConstructor,
            Self::SetConstructor => NativeCallable::SetConstructor,
            Self::WeakMapConstructor => NativeCallable::WeakMapConstructor,
            Self::WeakSetConstructor => NativeCallable::WeakSetConstructor,
            Self::WeakRefConstructor => NativeCallable::WeakRefConstructor,
            Self::FinalizationRegistryConstructor => NativeCallable::FinalizationRegistryConstructor,
            Self::DateConstructorGlobal => NativeCallable::DateConstructorGlobal,
            Self::PromiseConstructor => NativeCallable::PromiseConstructor,
            Self::ArrayBufferConstructorGlobal => NativeCallable::ArrayBufferConstructorGlobal,
            Self::DataViewConstructorGlobal => NativeCallable::DataViewConstructorGlobal,
            Self::BigInt64ArrayConstructor => NativeCallable::BigInt64ArrayConstructor,
            Self::BigUint64ArrayConstructor => NativeCallable::BigUint64ArrayConstructor,
            Self::ProxyConstructor => NativeCallable::ProxyConstructor,
            Self::GcCollect => NativeCallable::GcCollect,
            Self::SharedArrayBufferConstructor => NativeCallable::SharedArrayBufferConstructor,
            Self::AtomicsGlobal => NativeCallable::AtomicsGlobal,
            Self::AgentStart => NativeCallable::AgentStart,
            Self::AgentBroadcast => NativeCallable::AgentBroadcast,
            Self::AgentReceiveBroadcast => NativeCallable::AgentReceiveBroadcast,
            Self::AgentGetReport => NativeCallable::AgentGetReport,
            Self::AgentReport => NativeCallable::AgentReport,
            Self::AgentSleep => NativeCallable::AgentSleep,
            Self::AgentMonotonicNow => NativeCallable::AgentMonotonicNow,
            Self::HeadersConstructor => NativeCallable::HeadersConstructor,
            Self::ResponseConstructor => NativeCallable::ResponseConstructor,
            Self::RequestConstructor => NativeCallable::RequestConstructor,
            Self::AbortControllerConstructor => NativeCallable::AbortControllerConstructor,
            Self::ReadableStreamConstructor => NativeCallable::ReadableStreamConstructor,
            Self::WritableStreamConstructor => NativeCallable::WritableStreamConstructor,
            Self::TransformStreamConstructor => NativeCallable::TransformStreamConstructor,
            Self::CountQueuingStrategyConstructor => NativeCallable::CountQueuingStrategyConstructor,
            Self::ByteLengthQueuingStrategyConstructor => {
                NativeCallable::ByteLengthQueuingStrategyConstructor
            }
            Self::StubGlobal => NativeCallable::StubGlobal(()),
            Self::NumberPrimitiveMethod => {
                NativeCallable::NumberPrimitiveMethod { method: 0 }
            }
            Self::ArgumentsStrictCalleeGetter => NativeCallable::ArgumentsStrictCalleeGetter,
            Self::TypedArrayConstructor => NativeCallable::TypedArrayConstructor(()),
        }
    }

    pub(crate) fn try_from_native_callable(
        nc: &NativeCallable,
    ) -> Result<Self> {
        let result = match nc {
            NativeCallable::EvalIndirect => Self::EvalIndirect,
            NativeCallable::AsyncIteratorProtoSymbolAsyncIterator => {
                Self::AsyncIteratorProtoSymbolAsyncIterator
            }
            NativeCallable::ArrayProtoValues => Self::ArrayProtoValues,
            NativeCallable::ArrayConstructor => Self::ArrayConstructor,
            NativeCallable::ObjectConstructor => Self::ObjectConstructor,
            NativeCallable::ObjectProtoToString => Self::ObjectProtoToString,
            NativeCallable::ObjectProtoValueOf => Self::ObjectProtoValueOf,
            NativeCallable::FunctionConstructor => Self::FunctionConstructor,
            NativeCallable::StringConstructor => Self::StringConstructor,
            NativeCallable::BooleanConstructor => Self::BooleanConstructor,
            NativeCallable::NumberConstructor => Self::NumberConstructor,
            NativeCallable::SymbolConstructor => Self::SymbolConstructor,
            NativeCallable::BigIntConstructor => Self::BigIntConstructor,
            NativeCallable::RegExpConstructor => Self::RegExpConstructor,
            NativeCallable::ErrorConstructor => Self::ErrorConstructor,
            NativeCallable::TypeErrorConstructor => Self::TypeErrorConstructor,
            NativeCallable::RangeErrorConstructor => Self::RangeErrorConstructor,
            NativeCallable::SyntaxErrorConstructor => Self::SyntaxErrorConstructor,
            NativeCallable::ReferenceErrorConstructor => Self::ReferenceErrorConstructor,
            NativeCallable::URIErrorConstructor => Self::URIErrorConstructor,
            NativeCallable::EvalErrorConstructor => Self::EvalErrorConstructor,
            NativeCallable::AggregateErrorConstructor => Self::AggregateErrorConstructor,
            NativeCallable::MapConstructor => Self::MapConstructor,
            NativeCallable::SetConstructor => Self::SetConstructor,
            NativeCallable::WeakMapConstructor => Self::WeakMapConstructor,
            NativeCallable::WeakSetConstructor => Self::WeakSetConstructor,
            NativeCallable::WeakRefConstructor => Self::WeakRefConstructor,
            NativeCallable::FinalizationRegistryConstructor => {
                Self::FinalizationRegistryConstructor
            }
            NativeCallable::DateConstructorGlobal => Self::DateConstructorGlobal,
            NativeCallable::PromiseConstructor => Self::PromiseConstructor,
            NativeCallable::ArrayBufferConstructorGlobal => Self::ArrayBufferConstructorGlobal,
            NativeCallable::DataViewConstructorGlobal => Self::DataViewConstructorGlobal,
            NativeCallable::BigInt64ArrayConstructor => Self::BigInt64ArrayConstructor,
            NativeCallable::BigUint64ArrayConstructor => Self::BigUint64ArrayConstructor,
            NativeCallable::ProxyConstructor => Self::ProxyConstructor,
            NativeCallable::GcCollect => Self::GcCollect,
            NativeCallable::SharedArrayBufferConstructor => Self::SharedArrayBufferConstructor,
            NativeCallable::AtomicsGlobal => Self::AtomicsGlobal,
            NativeCallable::AgentStart => Self::AgentStart,
            NativeCallable::AgentBroadcast => Self::AgentBroadcast,
            NativeCallable::AgentReceiveBroadcast => Self::AgentReceiveBroadcast,
            NativeCallable::AgentGetReport => Self::AgentGetReport,
            NativeCallable::AgentReport => Self::AgentReport,
            NativeCallable::AgentSleep => Self::AgentSleep,
            NativeCallable::AgentMonotonicNow => Self::AgentMonotonicNow,
            NativeCallable::HeadersConstructor => Self::HeadersConstructor,
            NativeCallable::ResponseConstructor => Self::ResponseConstructor,
            NativeCallable::RequestConstructor => Self::RequestConstructor,
            NativeCallable::AbortControllerConstructor => Self::AbortControllerConstructor,
            NativeCallable::ReadableStreamConstructor => Self::ReadableStreamConstructor,
            NativeCallable::WritableStreamConstructor => Self::WritableStreamConstructor,
            NativeCallable::TransformStreamConstructor => Self::TransformStreamConstructor,
            NativeCallable::CountQueuingStrategyConstructor => {
                Self::CountQueuingStrategyConstructor
            }
            NativeCallable::ByteLengthQueuingStrategyConstructor => {
                Self::ByteLengthQueuingStrategyConstructor
            }
            NativeCallable::StubGlobal(()) => Self::StubGlobal,
            NativeCallable::ArgumentsStrictCalleeGetter => Self::ArgumentsStrictCalleeGetter,
            NativeCallable::NumberPrimitiveMethod { method: _ } => {
                Self::NumberPrimitiveMethod
            }
            NativeCallable::TypedArrayConstructor(()) => Self::TypedArrayConstructor,
            // 禁止捕获含运行态 handle 的变体
            NativeCallable::EvalFunction(_)
            | NativeCallable::PromiseResolvingFunction { .. }
            | NativeCallable::PromiseCombinatorReaction { .. }
            | NativeCallable::AsyncGeneratorMethod { .. }
            | NativeCallable::AsyncGeneratorIdentity { .. }
            | NativeCallable::ArrayLikeIteratorNext { .. }
            | NativeCallable::AsyncFromSyncNext { .. }
            | NativeCallable::AsyncFromSyncReturn { .. }
            | NativeCallable::AsyncFromSyncThrow { .. }
            | NativeCallable::MapSetMethod { .. }
            | NativeCallable::DateMethod { .. }
            | NativeCallable::WeakMapMethod { .. }
            | NativeCallable::WeakSetMethod { .. }
            | NativeCallable::WeakRefDerefMethod
            | NativeCallable::FinalizationRegistryRegisterMethod
            | NativeCallable::FinalizationRegistryUnregisterMethod
            | NativeCallable::ProxyRevoker { .. }
            | NativeCallable::HeadersMethod { .. }
            | NativeCallable::ResponseMethod { .. }
            | NativeCallable::RequestMethod { .. }
            | NativeCallable::AbortControllerAbort { .. }
            | NativeCallable::ReadableStreamMethod { .. }
            | NativeCallable::ReadableStreamDefaultReaderMethod { .. }
            | NativeCallable::ReadableStreamDefaultControllerMethod { .. }
            | NativeCallable::ReadableStreamByobRequestMethod { .. }
            | NativeCallable::ReadableStreamAsyncIteratorNext { .. }
            | NativeCallable::ReadableStreamAsyncIteratorReturn { .. }
            | NativeCallable::WritableStreamMethod { .. }
            | NativeCallable::WritableStreamDefaultWriterMethod { .. }
            | NativeCallable::WritableStreamDefaultControllerMethod { .. }
            | NativeCallable::TransformStreamMethod { .. }
            | NativeCallable::QueuingStrategySize { .. } => {
                bail!(
                    "SnapshotNativeCallable: unsupported runtime-state-carrying variant"
                )
            }
        };
        Ok(result)
    }
}

// ── encode ─────────────────────────────────────────────────────────

const SK_OBJECT_BYTES: u32 = 1;
const SK_HANDLE_OFFSETS: u32 = 2;
const SK_RUNTIME_STRINGS: u32 = 3;
const SK_NATIVE_CALLABLES: u32 = 4;
const SECTION_COUNT: u32 = 4;

pub(crate) fn encode_snapshot(snapshot: &StartupSnapshotOwned) -> Vec<u8> {
    // 两次写入: 先算 offset，再写最终 buf。
    let header_bytes = build_header_bytes(&snapshot.header);

    // section payloads (pre-serialized)
    let (obj_payload, ho_payload, rs_payload, nc_payload) = build_section_payloads(snapshot);

    // compute offsets
    let header_size = header_bytes.len() as u32; // 68
    let st_start = align_up(header_size, 4); // 72
    let st_size = (SECTION_COUNT * 12) as u32; // 48
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
    write_section_entry(&mut buf, SK_OBJECT_BYTES, obj_start, obj_payload.len() as u32);
    write_section_entry(&mut buf, SK_HANDLE_OFFSETS, ho_start, ho_payload.len() as u32);
    write_section_entry(&mut buf, SK_RUNTIME_STRINGS, rs_start, rs_payload.len() as u32);
    write_section_entry(&mut buf, SK_NATIVE_CALLABLES, nc_start, nc_payload.len() as u32);

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
    let mut b = Vec::with_capacity(68);
    b.extend_from_slice(&header.magic);
    b.extend_from_slice(&header.format_version.to_le_bytes());
    b.extend_from_slice(&header.abi_hash.to_le_bytes());
    b.extend_from_slice(&header.heap_used.to_le_bytes());
    b.extend_from_slice(&header.obj_table_count.to_le_bytes());
    b.extend_from_slice(&header.function_props_base.to_le_bytes());
    b.extend_from_slice(&header.object_proto_handle.to_le_bytes());
    b.extend_from_slice(&header.array_proto_handle.to_le_bytes());
    b.extend_from_slice(&header.async_iterator_prototype.to_le_bytes());
    b.extend_from_slice(&header.async_gen_prototype.to_le_bytes());
    b.extend_from_slice(&header.array_proto_values.to_le_bytes());
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
        let b = s.as_bytes();
        rs_payload.extend_from_slice(&(b.len() as u32).to_le_bytes());
        rs_payload.extend_from_slice(b);
    }

    let mut nc_payload = Vec::new();
    nc_payload.extend_from_slice(&(snapshot.native_callables.len() as u32).to_le_bytes());
    for nc in &snapshot.native_callables {
        nc_payload.extend_from_slice(&(*nc as u32).to_le_bytes());
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
    while (buf.len() % 4) != 0 {
        buf.push(0);
    }
}

// ── decode ─────────────────────────────────────────────────────────

/// Decode a snapshot from bytes. `object_bytes` 安全借用输入 bytes，
/// 其余字段 owned；不再使用 `unsafe`/`leak`/`from_raw_parts`。
/// `runtime_strings` 以拥有 `String` 返回，调用方直接用于 `RuntimeState`。
pub(crate) fn decode_snapshot(bytes: &[u8]) -> Result<StartupSnapshotView<'_>> {
    if bytes.len() < 68 {
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
    let async_iterator_prototype = i64::from_le_bytes(bytes[40..48].try_into()?);
    let async_gen_prototype = i64::from_le_bytes(bytes[48..56].try_into()?);
    let array_proto_values = i64::from_le_bytes(bytes[56..64].try_into()?);

    let section_count = u32::from_le_bytes(bytes[64..68].try_into()?) as usize;
    if section_count > 16 {
        bail!("too many sections: {}", section_count);
    }

    let header = StartupSnapshotHeader {
        magic,
        format_version,
        abi_hash,
        heap_used,
        obj_table_count,
        function_props_base,
        object_proto_handle,
        array_proto_handle,
        async_iterator_prototype,
        async_gen_prototype,
        array_proto_values,
    };

    // section table starts at offset 68 (header_bytes = 68, already 4-byte aligned)
    let st_start = 68;
    if bytes.len() < st_start + section_count * 12 {
        bail!("section table truncated");
    }

    let mut object_bytes: &[u8] = &[];
    let mut handle_rel_offsets: Vec<u32> = Vec::new();
    let mut runtime_strings: Vec<String> = Vec::new();
    let mut native_callables: Vec<SnapshotNativeCallable> = Vec::new();
    let mut seen_object = false;
    let mut seen_handles = false;
    let mut seen_strings = false;
    let mut seen_native = false;

    for i in 0..section_count {
        let off = st_start + i * 12;
        let _kind = u32::from_le_bytes(bytes[off..off + 4].try_into()?);
        let sect_off = u32::from_le_bytes(bytes[off + 4..off + 8].try_into()?) as usize;
        let sect_len = u32::from_le_bytes(bytes[off + 8..off + 12].try_into()?) as usize;
        if sect_off.checked_add(sect_len).map_or(true, |e| e > bytes.len()) {
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
                let mut strings: Vec<String> = Vec::with_capacity(count);
                let mut pos = 4usize;
                for _ in 0..count {
                    if pos + 4 > data.len() {
                        bail!("runtime_strings entry truncated");
                    }
                    let slen = u32::from_le_bytes(data[pos..pos + 4].try_into()?) as usize;
                    pos += 4;
                    if pos + slen > data.len() {
                        bail!("runtime_strings entry body truncated");
                    }
                    let s = std::str::from_utf8(&data[pos..pos + slen])?;
                    strings.push(s.to_string());
                    pos += slen;
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
                for j in 0..count {
                    let d = u32::from_le_bytes(data[4 + j * 4..8 + j * 4].try_into()?);
                    let nc = SnapshotNativeCallable::from_discriminant(d).ok_or_else(|| {
                        anyhow::anyhow!("unknown native callable discriminant {}", d)
                    })?;
                    ncs.push(nc);
                }
                native_callables = ncs;
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
    })
}

// ── ABI hash ────────────────────────────────────────────────────────

pub(crate) fn abi_hash() -> u64 {
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
    for d in 0u32..=57 {
        if let Some(_nc) = SnapshotNativeCallable::from_discriminant(d) {
            // hash the discriminant
            d.hash(&mut hasher);
        }
    }

    // Property slot constants
    constants::PROP_SLOT_SIZE.hash(&mut hasher);
    constants::FLAG_CONFIGURABLE.hash(&mut hasher);
    constants::FLAG_ENUMERABLE.hash(&mut hasher);
    constants::FLAG_WRITABLE.hash(&mut hasher);
    constants::FLAG_IS_ACCESSOR.hash(&mut hasher);

    hasher.finish()
}

fn align_up(n: u32, align: u32) -> u32 {
    (n + align - 1) & !(align - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn dummy_snapshot() -> StartupSnapshotOwned {
        StartupSnapshotOwned {
            header: StartupSnapshotHeader {
                magic: SNAPSHOT_MAGIC,
                format_version: SNAPSHOT_FORMAT_VERSION,
                abi_hash: abi_hash(),
                heap_used: 256,
                obj_table_count: 10,
                function_props_base: 5,
                object_proto_handle: 1,
                array_proto_handle: 2,
                async_iterator_prototype: wjsm_ir::value::encode_undefined(),
                async_gen_prototype: wjsm_ir::value::encode_undefined(),
                array_proto_values: wjsm_ir::value::encode_undefined(),
            },
            object_bytes: vec![0xAA; 256],
            handle_rel_offsets: vec![0, 16, 32, 48, 64, 80, 96, 112, 128, 144],
            runtime_strings: vec!["hello".to_string(), "world".to_string()],
            native_callables: vec![
                SnapshotNativeCallable::ObjectConstructor,
                SnapshotNativeCallable::ArrayConstructor,
            ],
        }
    }

    #[test]
    fn encode_decode_roundtrip() {
        let snap = dummy_snapshot();
        let bytes = encode_snapshot(&snap);
        let view = decode_snapshot(&bytes).expect("decode");

        assert_eq!(view.header.heap_used, 256);
        assert_eq!(view.header.obj_table_count, 10);
        assert_eq!(view.header.function_props_base, 5);
        assert_eq!(view.object_bytes, &[0xAA; 256]);
        assert_eq!(view.handle_rel_offsets.len(), 10);
        assert_eq!(view.handle_rel_offsets[0], 0);
        assert_eq!(view.handle_rel_offsets[9], 144);
        assert_eq!(view.runtime_strings, vec!["hello", "world"]);
        assert_eq!(view.native_callables.len(), 2);
    }

    #[test]
    fn decode_rejects_corrupt_magic() {
        let snap = dummy_snapshot();
        let mut bytes = encode_snapshot(&snap);
        for b in bytes.iter_mut().take(8) {
            *b = 0;
        }
        assert!(decode_snapshot(&bytes).is_err());
    }

    #[test]
    fn decode_rejects_object_bytes_length_mismatch() {
        let mut snap = dummy_snapshot();
        // 让 header.heap_used 与 object_bytes.len 不一致，decode 必须 bail。
        snap.header.heap_used = 128;
        let bytes = encode_snapshot(&snap);
        let err = decode_snapshot(&bytes).expect_err("expected length mismatch");
        let msg = format!("{err}");
        assert!(
            msg.contains("object_bytes len") || msg.contains("heap_used"),
            "diagnostic mentions length: {msg}"
        );
    }

    #[test]
    fn decode_rejects_out_of_range_handle_rel() {
        let mut snap = dummy_snapshot();
        // 让某个 handle rel 越过 heap_used，decode 必须 bail。
        snap.handle_rel_offsets[0] = snap.header.heap_used + 4;
        let bytes = encode_snapshot(&snap);
        let err = decode_snapshot(&bytes).expect_err("expected rel-bounds bail");
        assert!(format!("{err}").contains("handle_rel_offsets"));
    }

    #[test]
    fn decode_handle_count_mismatch() {
        // 篡改 header.obj_table_count 使其不等于 handle_rel_offsets 实际数量
        let mut snap = dummy_snapshot();
        snap.header.obj_table_count = 99; // 实际只有 10 个
        let bytes = encode_snapshot(&snap);
        assert!(decode_snapshot(&bytes).is_err());
    }

    #[test]
    fn decode_truncated_handle_section() {
        // 截断 handle section 使其长度不是 4 的倍数 + 数量不匹配
        let snap = dummy_snapshot();
        let mut bytes = encode_snapshot(&snap);
        // 找到 handle section 并截断末尾几个字节（破坏最后一个 u32）
        // header(68) + section_table(48) = 116 是 payload 起点
        // object_bytes section 在前（256 bytes + padding）
        // 直接截断文件末尾，使 handle section 的数据不完整
        // 更简单：直接删掉最后 4 字节，handle_rel_offsets 变成 9 个 ≠ obj_table_count=10
        bytes.truncate(bytes.len() - 4);
        // 截断后 native_callables section 也被破坏，但 decode 按顺序处理，
        // handle section 如果数量减少会触发 count mismatch
        // 如果截断点在 native_callables 区域，则 native_callables 会报 truncated
        // 无论哪种都是 Err
        assert!(decode_snapshot(&bytes).is_err());
    }

    #[test]
    fn decode_bad_magic() {
        let mut bytes = encode_snapshot(&dummy_snapshot());
        bytes[0] = b'X';
        assert!(decode_snapshot(&bytes).is_err());
    }

    #[test]
    fn decode_bad_version() {
        let mut snap = dummy_snapshot();
        snap.header.format_version = 99;
        let bytes = encode_snapshot(&snap);
        assert!(decode_snapshot(&bytes).is_err());
    }

    #[test]
    fn decode_too_short() {
        let bytes = vec![0; 8];
        assert!(decode_snapshot(&bytes).is_err());
    }

    #[test]
    fn try_from_runtime_callable_rejected() {
        use crate::types::PromiseResolvingKind;
        let nc = NativeCallable::PromiseResolvingFunction {
            promise: wjsm_ir::value::encode_undefined(),
            already_resolved: Arc::new(Mutex::new(false)),
            kind: PromiseResolvingKind::Fulfill,
        };
        assert!(SnapshotNativeCallable::try_from_native_callable(&nc).is_err());
    }

    #[test]
    fn try_from_stateless_callable_accepted() {
        let nc = NativeCallable::ArrayConstructor;
        assert!(SnapshotNativeCallable::try_from_native_callable(&nc).is_ok());
    }

    #[test]
    fn all_discriminants_roundtrip() {
        for d in 0..=57u32 {
            if let Some(snap_nc) = SnapshotNativeCallable::from_discriminant(d) {
                let native = snap_nc.into_native_callable();
                let back = SnapshotNativeCallable::try_from_native_callable(&native)
                    .expect("roundtrip");
                assert_eq!(snap_nc, back, "discriminant {d} roundtrip failed");
            }
        }
    }

    #[test]
    fn abi_hash_deterministic() {
        let h1 = abi_hash();
        let h2 = abi_hash();
        assert_eq!(h1, h2);
    }

    #[test]
    fn abi_hash_nonzero() {
        assert_ne!(abi_hash(), 0);
    }
}
