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
}

impl MapEntry {
    pub(crate) fn new_unowned() -> Self {
        Self {
            owner: None,
            keys: Vec::new(),
            values: Vec::new(),
        }
    }

    pub(crate) fn clear_for_reuse(&mut self) {
        self.owner = None;
        self.keys.clear();
        self.values.clear();
    }
}

pub(crate) struct SetEntry {
    pub(crate) owner: Option<u32>,
    pub(crate) values: Vec<i64>,
}

impl SetEntry {
    pub(crate) fn new_unowned() -> Self {
        Self {
            owner: None,
            values: Vec::new(),
        }
    }

    pub(crate) fn clear_for_reuse(&mut self) {
        self.owner = None;
        self.values.clear();
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum ResponseType {
    Basic,
    Cors,
    Error,
    Opaque,
    OpaqueRedirect,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RedirectMode {
    Follow,
    Error,
    Manual,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(dead_code)]
pub(crate) enum HeadersGuard {
    #[default]
    None,
    Request,
    RequestNoCors,
    Response,
    Immutable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(dead_code)]
pub(crate) enum RequestMode {
    #[default]
    Cors,
    SameOrigin,
    NoCors,
    Navigate,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(dead_code)]
pub(crate) enum RequestCredentials {
    #[default]
    SameOrigin,
    Omit,
    Include,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(dead_code)]
pub(crate) enum RequestCache {
    #[default]
    Default,
    NoStore,
    Reload,
    NoCache,
    ForceCache,
    OnlyIfCached,
}
#[derive(Clone, Debug)]
pub(crate) struct HeadersEntry {
    /// Lowercased key → value (append allows multi-value; we store duplicates)
    pub(crate) pairs: Vec<(String, String)>,
    #[allow(dead_code)]
    pub(crate) guard: HeadersGuard,
}
#[derive(Clone, Debug)]
pub(crate) struct FetchResponseEntry {
    pub(crate) status: u16,
    pub(crate) status_text: String,
    pub(crate) headers_handle: u32,
    /// 与 headers_handle 对应的 JS Headers wrapper；由 GC 追踪。
    pub(crate) headers_object: Option<i64>,
    pub(crate) url: String,
    pub(crate) body: Vec<u8>,
    pub(crate) response_type: ResponseType,
    pub(crate) redirected: bool,
    pub(crate) body_used: bool,
    pub(crate) http_response_handle: Option<u32>,
    /// body ReadableStream 在 readable_stream_table 中的 handle（用于 locked 检查）
    pub(crate) stream_handle: Option<u32>,
}
#[derive(Clone, Debug)]
pub(crate) struct FetchRequestEntry {
    pub(crate) method: String,
    pub(crate) url: String,
    pub(crate) headers_handle: u32,
    /// 与 headers_handle 对应的 JS Headers wrapper；由 GC 追踪。
    pub(crate) headers_object: Option<i64>,
    pub(crate) body: Option<Vec<u8>>,
    pub(crate) redirect: RedirectMode,
    #[allow(dead_code)]
    pub(crate) body_used: bool,
    #[allow(dead_code)]
    pub(crate) signal_handle: Option<u32>,
    // Extended observable fields per Fetch Standard
    #[allow(dead_code)]
    pub(crate) mode: RequestMode,
    #[allow(dead_code)]
    pub(crate) credentials: RequestCredentials,
    #[allow(dead_code)]
    pub(crate) cache: RequestCache,
    #[allow(dead_code)]
    pub(crate) referrer: String,
    #[allow(dead_code)]
    pub(crate) referrer_policy: String,
    #[allow(dead_code)]
    pub(crate) integrity: String,
    #[allow(dead_code)]
    pub(crate) keepalive: bool,
    #[allow(dead_code)]
    pub(crate) destination: String,
    #[allow(dead_code)]
    pub(crate) duplex: String,
}

#[derive(Clone, Debug)]
pub(crate) struct AbortSignalEntry {
    pub(crate) aborted: bool,
    pub(crate) reason: Option<i64>,
}

#[derive(Debug)]
pub(crate) struct HttpResponseEntry {
    pub response: Option<reqwest::Response>,
    pub pending_read_promise: Option<i64>,
    pub pending_bytes: std::collections::VecDeque<Vec<u8>>,
    pub eof: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum StreamState {
    Readable,
    Closed,
    Errored,
}

#[derive(Clone, Debug)]
pub(crate) struct ReadableStreamEntry {
    pub(crate) state: StreamState,
    pub(crate) error: Option<String>,
    pub(crate) disturbed: bool,
    pub(crate) locked: bool,
    pub(crate) http_response_handle: Option<u32>,
    /// 该流作为 Response.body 暴露时，对应的 Fetch Response 侧表 handle
    pub(crate) response_body_handle: Option<u32>,
    /// 该流作为 Response.body 暴露时，对应的 Response JS 对象
    pub(crate) response_body_object: Option<i64>,
    /// 关联的 controller handle（自定义流使用；HTTP 流为 None）
    pub(crate) controller_handle: Option<u32>,
    /// 是否为 byte stream（Phase 3 BYOB 支持预留）
    pub(crate) is_byte_stream: bool,
    pub(crate) pipe_to: Option<ReadableStreamPipeToEntry>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ReadableStreamPipeToEntry {
    pub(crate) destination: u32,
    pub(crate) promise: i64,
    pub(crate) write_in_flight: bool,
    pub(crate) closing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ReaderKind {
    Default,
    Byob,
}

#[derive(Clone, Debug)]
pub(crate) struct ReaderEntry {
    pub(crate) stream_handle: u32,
    pub(crate) kind: ReaderKind,
    /// 等待 enqueue 的 read Promise（自定义流路径使用）
    pub(crate) pending_read_promise: Option<i64>,
    /// BYOB read(view) 等待填充的目标 view
    pub(crate) pending_byob_view: Option<i64>,
    /// reader.closed Promise
    pub(crate) closed_promise: Option<i64>,
}
/// WritableStream 状态
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum WritableStreamState {
    Writable,
    Closing,
    Closed,
    Errored,
}
/// WritableStream 侧表条目
#[derive(Debug, Clone)]
pub(crate) struct WritableStreamEntry {
    pub(crate) state: WritableStreamState,
    pub(crate) error: Option<i64>,
    pub(crate) locked: bool,
    pub(crate) controller_handle: Option<u32>,
    pub(crate) abort_signal: Option<i64>,
}
/// WritableStreamDefaultWriter 侧表条目
#[derive(Debug, Clone)]
pub(crate) struct WriterEntry {
    pub(crate) writable_stream_handle: u32,
    pub(crate) closed_promise: Option<i64>,
    pub(crate) ready_promise: Option<i64>,
}
/// TransformStream 侧表条目
#[derive(Debug, Clone)]
pub(crate) struct TransformStreamEntry {
    pub(crate) readable_stream_handle: Option<u32>,
    pub(crate) writable_stream_handle: Option<u32>,
    pub(crate) transform_callback: Option<i64>,
    pub(crate) flush_callback: Option<i64>,
    pub(crate) readable_controller_handle: Option<u32>,
    /// transformer 对象（作为 transform/flush 回调的 this 值）
    pub(crate) transformer_this: Option<i64>,
    #[allow(dead_code)]
    pub(crate) backpressure: bool,
    /// readable JS 对象缓存（getter 返回用）
    pub(crate) readable_obj: Option<i64>,
    /// writable JS 对象缓存（getter 返回用）
    pub(crate) writable_obj: Option<i64>,
}

/// Controller 类型
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum ControllerKind {
    ReadableDefault,
    Writable,
    // 后续 Phase 使用：
    // ReadableByteStream,
    // Transform,
}

/// Stream Controller 侧表条目
#[derive(Clone)]
pub(crate) struct StreamControllerEntry {
    #[allow(dead_code)]
    pub(crate) kind: ControllerKind,
    pub(crate) stream_handle: u32,
    /// 排队的 chunk（NaN-boxed JS values）
    pub(crate) chunk_queue: VecDeque<i64>,
    pub(crate) high_water_mark: f64,
    pub(crate) strategy_size: Option<i64>,
    pub(crate) started: bool,
    pub(crate) close_requested: bool,

    #[allow(dead_code)]
    pub(crate) byob_reader_handle: Option<u32>,

    #[allow(dead_code)]
    pub(crate) pull_requested: bool,

    #[allow(dead_code)]
    pub(crate) abort_requested: bool,

    #[allow(dead_code)]
    pub(crate) abort_reason: Option<i64>,

    #[allow(dead_code)]
    pub(crate) flush_requested: bool,

    /// underlyingSource 对象（JS 值，GC root）
    pub(crate) underlying_source: Option<i64>,
    /// underlyingSource.pull 回调（JS callable）
    pub(crate) pull_callback: Option<i64>,
    /// underlyingSink.write 回调（Writable controller）
    pub(crate) write_callback: Option<i64>,
    /// underlyingSink.close 回调（Writable controller）
    pub(crate) sink_close_callback: Option<i64>,
    /// underlyingSource.cancel 回调（JS callable）
    pub(crate) cancel_callback: Option<i64>,
    /// 当前活动的 BYOB request handle（指向 byob_request_table）
    pub(crate) active_byob_request: Option<u32>,
}

#[derive(Clone, Debug)]
pub(crate) struct ByobRequestEntry {
    pub controller_handle: u32,
    pub reader_handle: u32,
    /// 用户提供的 Uint8Array view
    pub view: i64,
    /// 等待 fulfill 的 read() promise
    pub promise: i64,
    /// 是否已调用 respond()
    pub responded: bool,
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

#[derive(Clone)]
pub(crate) enum NativeCallable {
    EvalIndirect,

    /// raw bigint handle 上 `n.toString(radix)` / `valueOf`；`method`: 0=toString, 1=valueOf
    BigIntPrimitiveMethod {
        method: u8,
    },
    /// raw f64 上 `n.toString()` 等；`method`: 0=toString, 1=valueOf, 2=toFixed, 3=toExponential, 4=toPrecision
    NumberPrimitiveMethod {
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
    TypedArrayConstructor(()),
    BigInt64ArrayConstructor,
    BigUint64ArrayConstructor,
    ProxyConstructor,
    ProxyRevoker {
        proxy_handle: u32,
    },
    ProcessCwd,
    ProcessExit,
    ProcessNextTick,
    ProcessStreamWrite {
        kind: crate::runtime_process::ProcessStreamKind,
    },
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
pub(crate) enum HeadersMethodKind {
    Get,
    Set,
    Has,
    Delete,
    Append,
    Entries,
    ForEach,
    Keys,
    Values,
}
#[derive(Clone, Copy)]
pub(crate) enum ResponseMethodKind {
    Text,
    Json,
    ArrayBuffer,
    Clone,
}
#[derive(Clone, Copy)]
pub(crate) enum RequestMethodKind {
    Clone,
}
// ── ReadableStream (WHATWG Streams Phase 1) method kinds ──
#[derive(Clone, Copy)]
pub(crate) enum ReadableStreamMethodKind {
    GetReader,
    GetLocked,
    Cancel,
    Tee,
    AsyncIterator,
    PipeTo,
    PipeThrough,
}
#[derive(Clone, Copy)]
pub(crate) enum ReadableStreamDefaultReaderMethodKind {
    Read,
    ReleaseLock,
    GetClosed,
}
#[derive(Clone, Copy)]
pub(crate) enum ReadableStreamDefaultControllerMethodKind {
    Enqueue,
    Close,
    Error,
    GetDesiredSize,
    GetByobRequest,
}
#[derive(Clone, Copy)]
pub(crate) enum ReadableStreamByobRequestMethodKind {
    #[allow(dead_code)]
    GetView,
    Respond,
}
// ── TransformStream (WHATWG Streams Phase 5) method kinds ──
#[derive(Clone, Copy, Debug)]
pub(crate) enum TransformStreamMethodKind {
    GetReadable,
    GetWritable,
}
// ── WritableStream (WHATWG Streams Phase 4) method kinds ──
#[derive(Clone, Copy)]
pub(crate) enum WritableStreamMethodKind {
    GetWriter,
    Abort,
    Close,
    GetLocked,
}
#[derive(Clone, Copy)]
pub(crate) enum WritableStreamDefaultWriterMethodKind {
    Write,
    Close,
    Abort,
    ReleaseLock,
    GetClosed,
    GetReady,
    GetDesiredSize,
}
#[derive(Clone, Copy)]
pub(crate) enum WritableStreamDefaultControllerMethodKind {
    Error,
    GetSignal,
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
}

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
    /// Headers 迭代：按 pairs 顺序产出 name 或 value
    HeadersKeyIter {
        headers_handle: u32,
        index: u32,
    },
    HeadersValueIter {
        headers_handle: u32,
        index: u32,
    },
    HeadersEntryIter {
        headers_handle: u32,
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
        }
    }

    pub(crate) fn rejected(reason: i64) -> Self {
        Self {
            state: PromiseState::Rejected(reason),
            fulfill_reactions: Vec::new(),
            reject_reactions: Vec::new(),
            handled: false,
            constructor_resolver: None,
            constructor_handle: None,
            is_promise: true,
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
    },
    PromiseResolveThenable {
        promise: i64,
        thenable: i64,
        then: i64,
    },
    MicrotaskCallback {
        callback: i64,
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
