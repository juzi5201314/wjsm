//! Type definitions for runtime side tables and internal structures
//!
//! This module contains all the entry types, enums, and internal state structures
//! used by the runtime. Separating types from execution logic improves locality
//! when adding new heap types or modifying internal representations.

use crate::runtime_string::RuntimeString;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use swc_core::ecma::ast as swc_ast;
use tokio::time::Instant;
pub(crate) use wjsm_host::{
    AbortSignalEntry, ByobRequestEntry, CjsRequireCacheTrapKind, FetchRequestEntry,
    FetchResponseEntry, HeadersEntry, HeadersMethodKind, ReadableStreamByobRequestMethodKind,
    ReadableStreamDefaultControllerMethodKind, ReadableStreamDefaultReaderMethodKind,
    ReadableStreamEntry, ReadableStreamMethodKind, RedirectMode, ReaderEntry, RequestMethodKind,
    ResponseMethodKind, ResponseType, SharedFetchResourceTiming, StreamControllerEntry,
    StreamState, TransformStreamEntry, TransformStreamMethodKind,
    WritableStreamDefaultControllerMethodKind, WritableStreamDefaultWriterMethodKind,
    WritableStreamEntry, WritableStreamMethodKind, WriterEntry,
};
#[cfg(test)]
pub(crate) use wjsm_host::{
    ControllerKind, ReadableStreamPipeToEntry, RequestCache, RequestCredentials, RequestMode,
};

/// 绑定函数记录
pub(crate) struct BoundRecord {
    pub(crate) target_func: i64,     // TAG_FUNCTION / TAG_CLOSURE / TAG_BOUND
    pub(crate) bound_this: i64,      // NaN-boxed
    pub(crate) bound_args: Vec<i64>, // NaN-boxed values
}

/// Symbol 条目
pub(crate) struct SymbolEntry {
    pub(crate) description: Option<String>,
    pub(crate) global_key: Option<String>,
}

/// Error 条目：存储 error 对象的 name 和 message
pub(crate) struct ErrorEntry {
    #[allow(dead_code)]
    pub(crate) name: String,
    pub(crate) message: String,
    pub(crate) value: i64,
}

pub(crate) struct MapEntry {
    pub(crate) owner: Option<u32>,
    pub(crate) keys: Vec<i64>,
    pub(crate) values: Vec<i64>,
    /// SameValueZero 稳定哈希 → 槽位索引（仅存活键）。哈希冲突时回退线性扫描。
    pub(crate) index: HashMap<u64, u32>,
    /// 平行删除标记（槽位保留以保证插入顺序，迭代/快照需跳过）。
    pub(crate) deleted: Vec<bool>,
    /// 存活键数（= size）。
    pub(crate) live_count: u32,
    /// 已删除槽位数（触发压缩阈值）。
    pub(crate) deleted_count: u32,
}

impl MapEntry {
    pub(crate) fn new_unowned() -> Self {
        Self {
            owner: None,
            keys: Vec::new(),
            values: Vec::new(),
            index: HashMap::new(),
            deleted: Vec::new(),
            live_count: 0,
            deleted_count: 0,
        }
    }

    pub(crate) fn clear_for_reuse(&mut self) {
        self.owner = None;
        self.keys.clear();
        self.values.clear();
        self.index.clear();
        self.deleted.clear();
        self.live_count = 0;
        self.deleted_count = 0;
    }
}

pub(crate) struct SetEntry {
    pub(crate) owner: Option<u32>,
    pub(crate) values: Vec<i64>,
    /// SameValueZero 稳定哈希 → 槽位索引（仅存活值）。哈希冲突时回退线性扫描。
    pub(crate) index: HashMap<u64, u32>,
    /// 平行删除标记（槽位保留以保证插入顺序，迭代/快照需跳过）。
    pub(crate) deleted: Vec<bool>,
    /// 存活值数（= size）。
    pub(crate) live_count: u32,
    /// 已删除槽位数（触发压缩阈值）。
    pub(crate) deleted_count: u32,
}

impl SetEntry {
    pub(crate) fn new_unowned() -> Self {
        Self {
            owner: None,
            values: Vec::new(),
            index: HashMap::new(),
            deleted: Vec::new(),
            live_count: 0,
            deleted_count: 0,
        }
    }

    pub(crate) fn clear_for_reuse(&mut self) {
        self.owner = None;
        self.values.clear();
        self.index.clear();
        self.deleted.clear();
        self.live_count = 0;
        self.deleted_count = 0;
    }
}

#[derive(Clone, Debug)]
pub(crate) struct WeakMapEntry {
    pub(crate) map: HashMap<u32, i64>,
}

#[derive(Clone, Debug)]
pub(crate) struct WeakSetEntry {
    pub(crate) set: HashSet<u32>,
}

#[derive(Clone, Debug)]
pub(crate) struct WeakRefEntry {
    pub(crate) target_handle: Option<u32>,
}

#[derive(Clone, Debug)]
pub(crate) struct FinalizationRegistryEntry {
    #[allow(dead_code)]
    pub(crate) object_handle: u32,
    #[allow(dead_code)]
    pub(crate) callback: i64,
    pub(crate) registrations: Vec<FinalizationRegistration>,
}

#[derive(Clone, Debug)]
pub(crate) struct FinalizationRegistration {
    #[allow(dead_code)]
    pub(crate) target_handle: u32,
    #[allow(dead_code)]
    pub(crate) held_value: i64,
    pub(crate) unregister_token: Option<i64>,
}

#[derive(Clone, Debug)]
pub(crate) struct ArrayBufferEntry {
    pub(crate) data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(crate) struct DataViewEntry {
    pub(crate) buffer_handle: u32,
    /// 规范内部槽 [[ViewedArrayBuffer]] 对应的 JS buffer 对象；由 GC 追踪。
    pub(crate) buffer_object: Option<i64>,
    pub(crate) byte_offset: u32,
    pub(crate) byte_length: u32,
    pub(crate) is_shared: bool,
}

#[derive(Clone, Debug)]

pub(crate) struct TypedArrayEntry {
    pub(crate) buffer_handle: u32,
    /// 规范内部槽 [[ViewedArrayBuffer]] 对应的 JS buffer 对象；由 GC 追踪。
    pub(crate) buffer_object: Option<i64>,
    pub(crate) byte_offset: u32,
    pub(crate) length: u32,
    pub(crate) element_size: u8,
    /// 0=Int, 1=Uint, 2=Clamped, 3=Float, 4=BigInt, 5=BigUint
    pub(crate) element_kind: u8,
    pub(crate) is_shared: bool,
}

#[derive(Debug)]
pub(crate) struct HttpResponseEntry {
    pub response: Option<reqwest::Response>,
    pub pending_read_promise: Option<i64>,
    pub pending_bytes: std::collections::VecDeque<Vec<u8>>,
    pub eof: bool,
    pub error: Option<String>,
    pub resource_timing: Option<SharedFetchResourceTiming>,
}


#[derive(Clone, Debug)]
pub(crate) struct ProxyEntry {
    pub(crate) target: i64,
    pub(crate) handler: i64,
    pub(crate) revoked: bool,
}

/// RegExp 条目
#[derive(Clone)]
pub(crate) struct RegexEntry {
    pub(crate) pattern: String,
    pub(crate) flags: String,
    pub(crate) compiled: regress::Regex,
    pub(crate) last_index: i64,
}

/// 闭包条目
pub(crate) struct ClosureEntry {
    pub(crate) func_idx: u32,
    pub(crate) env_obj: i64,
}

/// Array/arguments 迭代器的产出种类：keys 产出下标，values 产出元素，entries 产出 [下标, 元素]。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArrayIterKind {
    Keys,
    Values,
    Entries,
}

#[derive(Clone, Copy)]
pub(crate) enum OsInfoKind {
    Tmpdir = 0,
    Homedir = 1,
    Hostname = 2,
    Cpus = 3,
    Totalmem = 4,
    Freemem = 5,
    Type = 6,
    Release = 7,
    Version = 8,
    NetworkInterfaces = 9,
}

impl OsInfoKind {
    pub(crate) fn from_method(method: u8) -> Option<Self> {
        match method {
            0 => Some(Self::Tmpdir),
            1 => Some(Self::Homedir),
            2 => Some(Self::Hostname),
            3 => Some(Self::Cpus),
            4 => Some(Self::Totalmem),
            5 => Some(Self::Freemem),
            6 => Some(Self::Type),
            7 => Some(Self::Release),
            8 => Some(Self::Version),
            9 => Some(Self::NetworkInterfaces),
            _ => None,
        }
    }

    pub(crate) fn method(self) -> u8 {
        self as u8
    }
}


#[derive(Clone)]
pub(crate) enum NativeCallable {
    EvalIndirect,

    CjsRequire {
        referrer: crate::RuntimeModuleReferrer,
    },
    CjsRequireResolve {
        referrer: crate::RuntimeModuleReferrer,
    },
    CjsRequireResolvePaths {
        referrer: crate::RuntimeModuleReferrer,
    },
    ImportMetaResolve {
        referrer: crate::RuntimeModuleReferrer,
    },
    CjsRequireCacheTrap {
        kind: CjsRequireCacheTrapKind,
    },

    /// raw bigint handle 上 `n.toString(radix)` / `valueOf`；`method`: 0=toString, 1=valueOf
    BigIntPrimitiveMethod {
        method: u8,
    },
    /// raw f64 上 `n.toString()` 等；`method`: 0=toString, 1=valueOf, 2=toFixed, 3=toExponential, 4=toPrecision
    NumberPrimitiveMethod {
        method: u8,
    },
    /// string primitive 上 String.prototype 方法；`method`: 0=includes, 1=startsWith, 2=indexOf, 3=slice, 4=concat
    StringPrimitiveMethod {
        method: u8,
    },
    /// symbol handle 上 Symbol.prototype 方法；`method`: 0=toString, 1=valueOf
    SymbolPrimitiveMethod {
        method: u8,
    },
    /// raw RegExp handle 上 RegExp.prototype/string-symbol 方法；method 定义在 runtime_regexp。
    RegExpPrimitiveMethod {
        method: u8,
    },
    /// Symbol.prototype.description getter
    SymbolProtoDescriptionGetter,
    /// Symbol.prototype[Symbol.toPrimitive]
    SymbolProtoToPrimitive,
    ArgumentsStrictCalleeGetter,
    EvalFunction(EvalFunction),
    PromiseResolvingFunction {
        promise: i64,
        already_resolved: Arc<Mutex<bool>>,
        kind: PromiseResolvingKind,
    },
    PromiseCombinatorReaction {
        context: usize,
        index: usize,
        kind: PromiseCombinatorReactionKind,
    },
    /// §27.2.5.4 Promise.prototype.finally：当 onFinally 返回 thenable 时挂在中间
    /// promise 上的 await 反应。inner promise settle 后按 finally 语义 settle target_promise：
    /// inner fulfill 时用 original_value（finally_is_reject 决定 fulfill/reject），
    /// inner reject 时用 inner 的 reason reject。
    PromiseFinallyAwait {
        target_promise: i64,
        original_value: i64,
        finally_is_reject: bool,
    },
    AsyncGeneratorMethod {
        generator: i64,
        kind: AsyncGeneratorCompletionType,
    },
    AsyncGeneratorIdentity {
        generator: i64,
    },
    GeneratorMethod {
        generator: i64,
        kind: GeneratorCompletionType,
    },
    GeneratorIdentity {
        generator: i64,
    },
    /// %AsyncIteratorPrototype%[Symbol.asyncIterator]() → return this
    AsyncIteratorProtoSymbolAsyncIterator,
    /// %IteratorPrototype%[Symbol.iterator]() → return this
    IteratorProtoSymbolIterator,
    /// RegExp String Iterator 的 next()：推进 RegExpStringIter 状态，返回 {value, done}。
    RegExpStringIteratorNext {
        iter_handle: u32,
    },
    /// RegExp String Iterator 的 [Symbol.iterator]() → return this。
    RegExpStringIteratorSelf,
    /// Array.prototype.values() / arguments @@iterator（产出元素）。
    ArrayProtoValues,
    /// Array.prototype.keys()（产出下标）。
    ArrayProtoKeys,
    /// Array.prototype.entries()（产出 [下标, 元素]）。
    ArrayProtoEntries,
    /// Array.prototype.toString()：Get(this, "join") 后调用或回落 Object.prototype.toString。
    ArrayProtoToString,
    ArrayLikeIteratorNext {
        target: i64,
        index: Arc<Mutex<u32>>,
        length: u32,
        kind: ArrayIterKind,
    },
    /// 内部 TAG_ITERATOR 包装对象的 next()。
    RawIteratorNext {
        iterator: i64,
    },

    /// AsyncFromSyncIterator.prototype.next()
    AsyncFromSyncNext {
        handle: u32,
    },
    /// AsyncFromSyncIterator.prototype.return()
    AsyncFromSyncReturn {
        handle: u32,
    },
    /// AsyncFromSyncIterator.prototype.throw()
    #[allow(dead_code)]
    AsyncFromSyncThrow {
        handle: u32,
    },
    MapSetMethod {
        kind: MapSetMethodKind,
    },
    DateMethod {
        kind: DateMethodKind,
    },
    WeakMapMethod {
        kind: WeakMapMethodKind,
    },
    WeakSetMethod {
        kind: WeakSetMethodKind,
    },
    WeakRefDerefMethod,
    FinalizationRegistryRegisterMethod,
    FinalizationRegistryUnregisterMethod,
    ArrayConstructor,
    ObjectConstructor,
    ErrorProtoToString,
    ObjectProtoToString,
    ObjectProtoValueOf,
    FunctionProtoCall,
    FunctionProtoApply,
    FunctionProtoBind,
    FunctionConstructor,
    StringConstructor,
    BooleanConstructor,
    NumberConstructor,
    SymbolConstructor,
    BigIntConstructor,
    RegExpConstructor,
    ErrorConstructor,
    TypeErrorConstructor,
    RangeErrorConstructor,
    SyntaxErrorConstructor,
    ReferenceErrorConstructor,
    URIErrorConstructor,
    EvalErrorConstructor,
    AggregateErrorConstructor,
    MapConstructor,
    SetConstructor,
    WeakMapConstructor,
    WeakSetConstructor,
    WeakRefConstructor,
    FinalizationRegistryConstructor,
    DateConstructorGlobal,
    PromiseConstructor,
    ArrayBufferConstructorGlobal,
    DataViewConstructorGlobal,
    TypedArrayConstructor(TypedArrayConstructorKind),
    BufferConstructor,
    BufferStatic {
        kind: BufferStaticKind,
    },
    BufferMethod {
        kind: BufferMethodKind,
    },
    TextEncoderConstructor,
    TextEncoderMethod {
        kind: TextEncoderMethodKind,
    },
    TextDecoderConstructor,
    TextDecoderMethod {
        kind: TextDecoderMethodKind,
    },
    StructuredClone,
    Atob,
    Btoa,
    QueueMicrotask,
    PerformanceNow,
    PerfHooksMethod {
        kind: crate::runtime_node_perf_hooks::PerfHooksMethodKind,
    },
    OsInfo {
        kind: OsInfoKind,
    },
    FsMethod {
        kind: crate::runtime_node_fs::FsMethodKind,
    },
    CryptoMethod {
        kind: crate::runtime_node_crypto::CryptoMethodKind,
    },
    ZlibMethod {
        kind: crate::runtime_node_zlib::ZlibMethodKind,
    },
    ChildProcessMethod {
        kind: crate::runtime_node_child_process::ChildProcessMethodKind,
    },
    NetMethod {
        kind: crate::runtime_node_net::NetMethodKind,
    },
    VmMethod {
        kind: crate::runtime_node_vm::VmMethodKind,
    },
    AsyncHooksMethod {
        kind: crate::runtime_node_async_hooks::AsyncHooksMethodKind,
    },
    DgramMethod {
        kind: crate::runtime_node_dgram::DgramMethodKind,
    },
    TlsMethod {
        kind: crate::runtime_node_tls::TlsMethodKind,
    },
    WorkerThreadsMethod {
        kind: crate::runtime_node_worker_threads::WorkerThreadsMethodKind,
    },
    CryptoDigestMethod {
        state: Arc<Mutex<crate::runtime_node_crypto::CryptoDigestState>>,
        kind: crate::runtime_node_crypto::CryptoDigestKind,
    },
    BigInt64ArrayConstructor,
    BigUint64ArrayConstructor,
    ProxyConstructor,
    ProxyRevoker {
        proxy_handle: u32,
    },
    ProcessCwd,
    ProcessExit,
    ProcessNextTick,
    /// IPC：`process.send(message[, sendHandle])`
    ProcessSend,
    /// IPC：`process.disconnect()`
    ProcessDisconnect,
    /// IPC / 事件：`process.on(event, listener)`（message/disconnect）
    ProcessOn,
    ProcessStreamWrite {
        kind: crate::runtime_process::ProcessStreamKind,
    },
    ProcessStreamEnd {
        kind: crate::runtime_process::ProcessStreamKind,
    },
    ProcessStreamOn {
        kind: crate::runtime_process::ProcessStreamKind,
    },
    ProcessStdinResume,
    ProcessHrtime,
    ProcessHrtimeBigint,
    ProcessMemoryUsage,
    ProcessUptime,
    ProcessCpuUsage,
    ProcessEnvTrap {
        kind: crate::runtime_process::ProcessEnvTrapKind,
    },
    /// GcCollect: trigger mark-sweep GC collection
    GcCollect,
    StubGlobal(()),
    // ── SharedArrayBuffer builtins ──
    SharedArrayBufferConstructor,
    // ── Atomics builtins ──
    AtomicsGlobal,
    // ── Agent harness ──
    AgentStart,
    AgentBroadcast,
    AgentReceiveBroadcast,
    AgentGetReport,
    AgentReport,
    AgentSleep,
    AgentMonotonicNow,
    // ── Fetch / Headers / Response / Request method dispatch ──
    HeadersMethod {
        #[allow(dead_code)]
        handle: u32,
        kind: HeadersMethodKind,
    },
    ResponseMethod {
        #[allow(dead_code)]
        handle: u32,
        kind: ResponseMethodKind,
    },
    RequestMethod {
        #[allow(dead_code)]
        handle: u32,
        kind: RequestMethodKind,
    },
    // Constructors for the Fetch API (installed on globalThis)
    HeadersConstructor,
    ResponseConstructor,
    RequestConstructor,
    // ── ReadableStream / Reader / AbortController ──
    AbortControllerConstructor,
    #[allow(dead_code)]
    AbortControllerAbort {
        signal_handle: u32,
    },
    // ── ReadableStream (WHATWG Streams Phase 1) ──
    ReadableStreamConstructor,
    ReadableStreamMethod {
        handle: u32,
        kind: ReadableStreamMethodKind,
    },
    ReadableStreamDefaultReaderMethod {
        handle: u32,
        kind: ReadableStreamDefaultReaderMethodKind,
    },
    ReadableStreamDefaultControllerMethod {
        handle: u32,
        kind: ReadableStreamDefaultControllerMethodKind,
    },
    ReadableStreamByobRequestMethod {
        handle: u32,
        kind: ReadableStreamByobRequestMethodKind,
    },
    // ── ReadableStream async iterator (WHATWG Streams Phase 2) ──
    /// ReadableStream async iterator next()
    ReadableStreamAsyncIteratorNext {
        reader_handle: u32,
    },
    /// ReadableStream async iterator return()
    ReadableStreamAsyncIteratorReturn {
        reader_handle: u32,
    },
    ReadableStreamPipeToWriteFulfilled {
        readable_handle: u32,
    },
    ReadableStreamPipeToWriteRejected {
        readable_handle: u32,
    },
    // ── WritableStream (WHATWG Streams Phase 4) ──
    /// WritableStream constructor
    WritableStreamConstructor,
    // ── TransformStream (WHATWG Streams Phase 5) ──
    /// TransformStream constructor
    TransformStreamConstructor,
    /// TransformStream method (readable getter, writable getter)
    TransformStreamMethod {
        handle: u32,
        kind: TransformStreamMethodKind,
    },
    /// WritableStream method (getWriter, abort, close, getLocked)
    WritableStreamMethod {
        handle: u32,
        kind: WritableStreamMethodKind,
    },
    /// WritableStreamDefaultWriter method (write, close, abort, closed getter, ready getter, desiredSize getter)
    WritableStreamDefaultWriterMethod {
        handle: u32,
        kind: WritableStreamDefaultWriterMethodKind,
    },
    /// WritableStreamDefaultController method (error)
    WritableStreamDefaultControllerMethod {
        handle: u32,
        kind: WritableStreamDefaultControllerMethodKind,
    },
    /// CountQueuingStrategy / ByteLengthQueuingStrategy constructor
    CountQueuingStrategyConstructor,
    ByteLengthQueuingStrategyConstructor,
    /// QueuingStrategy size(chunk) method
    QueuingStrategySize {
        kind: QueuingStrategySizeKind,
    },
    /// Object.* 静态方法（作为可获取函数值）
    ObjectStatic {
        kind: ObjectStaticKind,
    },
    /// Promise.* 静态方法（作为可获取函数值）
    PromiseStatic {
        kind: PromiseStaticKind,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TypedArrayConstructorKind {
    Int8,
    Uint8,
    Uint8Clamped,
    Int16,
    Uint16,
    Int32,
    Uint32,
    Float32,
    Float64,
    BigInt64,
    BigUint64,
}

impl TypedArrayConstructorKind {
    pub(crate) const COUNT: usize = 11;

    pub(crate) fn index(self) -> usize {
        match self {
            Self::Int8 => 0,
            Self::Uint8 => 1,
            Self::Uint8Clamped => 2,
            Self::Int16 => 3,
            Self::Uint16 => 4,
            Self::Int32 => 5,
            Self::Uint32 => 6,
            Self::Float32 => 7,
            Self::Float64 => 8,
            Self::BigInt64 => 9,
            Self::BigUint64 => 10,
        }
    }

    pub(crate) fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Int8),
            1 => Some(Self::Uint8),
            2 => Some(Self::Uint8Clamped),
            3 => Some(Self::Int16),
            4 => Some(Self::Uint16),
            5 => Some(Self::Int32),
            6 => Some(Self::Uint32),
            7 => Some(Self::Float32),
            8 => Some(Self::Float64),
            9 => Some(Self::BigInt64),
            10 => Some(Self::BigUint64),
            _ => None,
        }
    }

    pub(crate) fn element(self) -> (u8, u8) {
        match self {
            Self::Int8 => (1, 0),
            Self::Uint8 => (1, 1),
            Self::Uint8Clamped => (1, 2),
            Self::Int16 => (2, 0),
            Self::Uint16 => (2, 1),
            Self::Int32 => (4, 0),
            Self::Uint32 => (4, 1),
            Self::Float32 => (4, 3),
            Self::Float64 => (8, 3),
            Self::BigInt64 => (8, 4),
            Self::BigUint64 => (8, 5),
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Int8 => "Int8Array",
            Self::Uint8 => "Uint8Array",
            Self::Uint8Clamped => "Uint8ClampedArray",
            Self::Int16 => "Int16Array",
            Self::Uint16 => "Uint16Array",
            Self::Int32 => "Int32Array",
            Self::Uint32 => "Uint32Array",
            Self::Float32 => "Float32Array",
            Self::Float64 => "Float64Array",
            Self::BigInt64 => "BigInt64Array",
            Self::BigUint64 => "BigUint64Array",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum BufferStaticKind {
    Alloc,
    AllocUnsafe,
    From,
    Concat,
    IsBuffer,
    ByteLength,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObjectStaticKind {
    Keys,
    Values,
    Entries,
    Assign,
    Create,
    GetPrototypeOf,
    SetPrototypeOf,
    GetOwnPropertyNames,
    Is,
    HasOwn,
    FromEntries,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PromiseStaticKind {
    Resolve,
    Reject,
    All,
    Race,
    AllSettled,
    Any,
    WithResolvers,
}

#[derive(Clone, Copy)]
pub(crate) enum TextEncoderMethodKind {
    Encode,
    EncodeInto,
}

#[derive(Clone, Copy)]
pub(crate) enum TextDecoderMethodKind {
    Decode,
}

#[derive(Clone, Copy)]
pub(crate) enum BufferMethodKind {
    ToString,
    Slice,
    Subarray,
    Copy,
    Compare,
    Write,
    ReadUInt8,
    ReadUInt16BE,
    ReadUInt16LE,
    ReadUInt32BE,
    ReadUInt32LE,
    ReadInt8,
    ReadInt16BE,
    ReadInt16LE,
    ReadInt32BE,
    ReadInt32LE,
    ReadFloatBE,
    ReadFloatLE,
    ReadDoubleBE,
    ReadDoubleLE,
    WriteUInt8,
    WriteUInt16BE,
    WriteUInt16LE,
    WriteUInt32BE,
    WriteUInt32LE,
    WriteInt8,
    WriteInt16BE,
    WriteInt16LE,
    WriteInt32BE,
    WriteInt32LE,
    WriteFloatBE,
    WriteFloatLE,
    WriteDoubleBE,
    WriteDoubleLE,
    Fill,
    IndexOf,
    Includes,
    ToJson,
    Equals,
}

#[derive(Clone, Copy)]
pub(crate) enum MapSetMethodKind {
    MapSet,
    MapGet,
    SetAdd,
    Has,
    Delete,
    Clear,
    Size,
    ForEach,
    Keys,
    Values,
    Entries,
}
#[derive(Clone, Copy)]
pub(crate) enum WeakMapMethodKind {
    Set,
    Get,
    Has,
    Delete,
}

#[derive(Clone, Copy)]
pub(crate) enum WeakSetMethodKind {
    Add,
    Has,
    Delete,
}

#[derive(Clone, Copy)]
pub(crate) enum DateMethodKind {
    GetDate,
    GetDay,
    GetFullYear,
    GetHours,
    GetMilliseconds,
    GetMinutes,
    GetMonth,
    GetSeconds,
    GetTime,
    GetTimezoneOffset,
    GetUTCDate,
    GetUTCDay,
    GetUTCFullYear,
    GetUTCHours,
    GetUTCMilliseconds,
    GetUTCMinutes,
    GetUTCMonth,
    GetUTCSeconds,
    SetDate,
    SetFullYear,
    SetHours,
    SetMilliseconds,
    SetMinutes,
    SetMonth,
    SetSeconds,
    SetTime,
    SetUTCDate,
    SetUTCFullYear,
    SetUTCHours,
    SetUTCMilliseconds,
    SetUTCMinutes,
    SetUTCMonth,
    SetUTCSeconds,
    ToString,
    ToDateString,
    ToTimeString,
    ToLocaleString,
    ToLocaleDateString,
    ToLocaleTimeString,
    ToISOString,
    ToUTCString,
    ToJSON,
    ValueOf,
}
#[derive(Clone, Copy)]
pub(crate) enum QueuingStrategySizeKind {
    Count,
    ByteLength,
}

#[derive(Clone, Copy)]
pub(crate) enum PromiseCombinatorReactionKind {
    AllFulfill,
    AllReject,
    AllSettledFulfill,
    AllSettledReject,
    AnyFulfill,
    AnyReject,
}
pub(crate) struct CombinatorContext {
    pub(crate) result_promise: i64,
    pub(crate) result_array: i64,
    pub(crate) remaining: usize,
    pub(crate) settled: bool,
    /// 已挂接到输入 Promise、但尚未观察到 fulfill/reject 其中一个分支的 pending 输入数。
    pub(crate) outstanding_settlements: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EvalVarMapEntry {
    pub(crate) function_name: String,
    pub(crate) var_name: String,
    pub(crate) offset: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EvalLocalKind {
    Var,
    Let,
    Const,
}

pub(crate) struct EvalLocalBinding {
    pub(crate) kind: EvalLocalKind,
    pub(crate) value: i64,
}

#[derive(Clone)]
pub(crate) struct EvalFunction {
    pub(crate) params: Vec<String>,
    pub(crate) body: Vec<swc_ast::Stmt>,
    pub(crate) scope_env: Option<i64>,
}

#[derive(Clone, Copy)]
pub(crate) enum PromiseResolvingKind {
    Fulfill,
    Reject,
}

pub(crate) struct TimerEntry {
    pub(crate) id: u32,
    pub(crate) deadline: Instant,
    pub(crate) callback: i64, // NaN-boxed function handle
    pub(crate) repeating: bool,
    pub(crate) interval: Duration,
    /// JS 可见 Timeout 对象（init.resource / setTimeout 返回值）
    #[allow(dead_code)]
    pub(crate) resource: i64,
    /// 调度时捕获的 async scope（hooks/ALS 开启时有效）
    pub(crate) scope: Option<crate::CapturedScope>,
}

/// setImmediate 队列条目。
pub(crate) struct ImmediateEntry {
    pub(crate) id: u32,
    pub(crate) callback: i64,
    #[allow(dead_code)]
    pub(crate) resource: i64,
    pub(crate) scope: Option<crate::CapturedScope>,
    pub(crate) native_performance_converter: Option<i64>,
    pub(crate) native_performance_dispatcher: Option<i64>,
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum IteratorState {
    StringIter {
        string: RuntimeString,
        unit_pos: usize,
    },
    ArrayIter {
        ptr: usize,
        index: u32,
        length: u32,
    },
    MapKeyIter {
        map_handle: u32,
        owner: i64,
        index: u32,
    },
    MapValueIter {
        map_handle: u32,
        owner: i64,
        index: u32,
    },
    /// Map [key, value] 对迭代
    MapEntryIter {
        map_handle: u32,
        owner: i64,
        index: u32,
    },
    /// Set 值迭代：读取 set_table.values，勿与 MapValueIter 混用
    SetValueIter {
        set_handle: u32,
        owner: i64,
        index: u32,
    },
    /// Set [value, value] 对迭代：Set.prototype.entries 专用。
    SetEntryIter {
        set_handle: u32,
        owner: i64,
        index: u32,
    },
    /// 预物化索引序列（如 TypedArray.prototype.keys 的 0..length）
    IndexValueIter {
        values: Vec<i64>,
        index: u32,
    },
    TypedArrayValueIter {
        entry: TypedArrayEntry,
        index: u32,
        length: u32,
    },
    TypedArrayEntryIter {
        entry: TypedArrayEntry,
        index: u32,
        length: u32,
    },
    RegExpStringIter {
        entry: RegexEntry,
        string: String,
        next_index: usize,
        current: Option<crate::runtime_regexp::RegExpStringMatchInfo>,
        done: bool,
    },
    ObjectIter {
        iterator: i64,
        next: i64,
        return_method: Option<i64>,
        throw_method: Option<i64>,
        current_value: i64,
        done: bool,
        has_current: bool,
    },
    Error,
}

pub(crate) enum EnumeratorState {
    StringEnum {
        length: usize,
        index: usize,
    },
    /// 对象属性枚举：keys 存储属性名列表
    ObjectEnum {
        keys: Vec<String>,
        index: usize,
    },
    Error,
}

#[derive(Clone)]
pub(crate) enum PromiseState {
    Pending,
    Fulfilled(i64),
    Rejected(i64),
}

#[derive(Clone)]
pub(crate) struct PromiseEntry {
    pub(crate) state: PromiseState,
    pub(crate) fulfill_reactions: Vec<PromiseReaction>,
    pub(crate) reject_reactions: Vec<PromiseReaction>,
    pub(crate) handled: bool,
    pub(crate) constructor_resolver: Option<Arc<Mutex<bool>>>,
    /// 构造器引用（用于 species-aware 操作；None 表示内建 Promise）
    pub(crate) constructor_handle: Option<i64>,
    pub(crate) is_promise: bool,
    /// 创建时捕获的 ALS/hooks scope（then 反应继承，非 then 注册时 current）
    pub(crate) capture_scope: Option<crate::CapturedScope>,
}

impl PromiseEntry {
    pub(crate) fn pending() -> Self {
        Self {
            state: PromiseState::Pending,
            fulfill_reactions: Vec::new(),
            reject_reactions: Vec::new(),
            handled: false,
            constructor_resolver: None,
            constructor_handle: None,
            is_promise: true,
            capture_scope: None,
        }
    }

    pub(crate) fn empty() -> Self {
        Self {
            state: PromiseState::Pending,
            fulfill_reactions: Vec::new(),
            reject_reactions: Vec::new(),
            handled: false,
            constructor_resolver: None,
            constructor_handle: None,
            is_promise: false,
            capture_scope: None,
        }
    }
}

#[derive(Clone)]
pub(crate) enum PromiseReactionKind {
    Normal { handler: i64 },
    AsyncResume { fn_table_idx: u32, state: u32 },
}

#[derive(Clone)]
pub(crate) struct PromiseReaction {
    pub(crate) kind: PromiseReactionKind,
    pub(crate) target_promise: i64,
    pub(crate) reaction_type: ReactionType,
}

impl PromiseReaction {
    pub(crate) fn new(handler: i64, target_promise: i64, reaction_type: ReactionType) -> Self {
        Self {
            kind: PromiseReactionKind::Normal { handler },
            target_promise,
            reaction_type,
        }
    }
    pub(crate) fn new_async(
        fn_table_idx: u32,
        target_promise: i64,
        reaction_type: ReactionType,
        state: u32,
    ) -> Self {
        Self {
            kind: PromiseReactionKind::AsyncResume {
                fn_table_idx,
                state,
            },
            target_promise,
            reaction_type,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ReactionType {
    Fulfill,
    Reject,
    FinallyFulfill,
    FinallyReject,
}

#[derive(Clone)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum Microtask {
    PromiseReaction {
        promise: i64,
        reaction_type: ReactionType,
        handler: i64,
        argument: i64,
        scope: Option<crate::CapturedScope>,
    },
    PromiseResolveThenable {
        promise: i64,
        thenable: i64,
        then: i64,
    },
    MicrotaskCallback {
        callback: i64,
        scope: Option<crate::CapturedScope>,
    },
    TransformStreamTransform {
        callback: i64,
        this_val: i64,
        chunk: i64,
        controller: i64,
        write_promise: i64,
    },
    TransformStreamFlush {
        callback: Option<i64>,
        this_val: i64,
        controller: i64,
        writable_stream_handle: u32,
        readable_stream_handle: u32,
        readable_controller_handle: u32,
        close_promise: i64,
    },
    ReadableStreamPipeToPump {
        readable_handle: u32,
    },
    AsyncResume {
        fn_table_idx: u32,
        continuation: i64,
        state: u32,
        resume_val: i64,
        completion: u8,
        scope: Option<crate::CapturedScope>,
    },
    #[allow(dead_code)]
    CleanupFinalizationRegistry {
        callback: i64,
        held_value: i64,
    },
    ReadableStreamPull {
        callback: i64,
        this_val: i64,
        controller: i64,
    },
    WritableStreamSinkWrite {
        callback: i64,
        this_val: i64,
        chunk: i64,
        controller: i64,
        write_promise: i64,
    },
    WritableStreamSinkClose {
        callback: Option<i64>,
        this_val: i64,
        controller: i64,
        writable_stream_handle: u32,
        close_promise: i64,
    },
}

#[derive(Clone)]

pub(crate) struct ContinuationEntry {
    pub(crate) fn_table_idx: u32,
    pub(crate) outer_promise: i64,
    pub(crate) captured_vars: Vec<i64>,
    pub(crate) completed: bool,
    /// 异步生成器 return(v) 在 yield 恢复前入队时，通过此标记通知
    /// resume_async_function_async 将 completion 覆盖为 2（return 语义）。
    pub(crate) pending_return: Option<i64>,
}

pub(crate) struct AsyncGeneratorEntry {
    pub(crate) state: AsyncGeneratorState,
    pub(crate) continuation: i64,
    pub(crate) active_request: Option<AsyncGeneratorRequest>,
    pub(crate) waiting_resume_promise: Option<i64>,
    pub(crate) queue: VecDeque<AsyncGeneratorRequest>,
}

#[derive(Clone)]

pub(crate) enum AsyncGeneratorState {
    SuspendedStart,
    SuspendedYield,
    Executing,
    Completed,
}
#[derive(Clone, Copy)]

pub(crate) struct AsyncGeneratorRequest {
    pub(crate) completion_type: AsyncGeneratorCompletionType,
    pub(crate) value: i64,
    pub(crate) promise: i64,
}

pub(crate) enum AsyncGeneratorHostAction {
    Immediate {
        active: Option<AsyncGeneratorRequest>,
        queued: VecDeque<AsyncGeneratorRequest>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AsyncGeneratorCompletionType {
    Next,
    Return,
    Throw,
}

pub(crate) struct GeneratorEntry {
    pub(crate) state: GeneratorState,
    pub(crate) continuation: i64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GeneratorState {
    SuspendedStart,
    SuspendedYield,
    Executing,
    Completed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GeneratorCompletionType {
    Next,
    Return,
    Throw,
}
/// async-from-sync iterator 内部状态
#[derive(Clone, Debug)]
pub(crate) struct AsyncFromSyncIteratorEntry {
    /// 同步迭代器句柄 (TAG_ITERATOR handle)
    pub(crate) sync_iterator: i64,
    /// 同步迭代器是否已完成
    pub(crate) sync_done: bool,
    /// for-await 使用的 AsyncFromSync 外层 TAG_ITERATOR 句柄
    pub(crate) outer_iter: i64,
    /// 外层 ObjectIter 在 iterators 表中的索引
    pub(crate) outer_handle_idx: u32,
}
