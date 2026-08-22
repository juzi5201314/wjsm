//! 后端无关的宿主环境（host environment）抽象。
//!
//! 本 crate 定义 JS runtime 的宿主能力 trait，不依赖具体 native image 实现。
//!
//! # 分层
//!
//! ```text
//! HeapContext                                       ← 堆/侧表最小操作集（解耦接缝）
//!        │
//! ExecContext: HeapContext                          ← builtins 完整能力（泛型单态化）
//!        │ 后端用自身运行时上下文实现
//!        ▼
//! native 后端的 `NativeExecContext` 实现这些 trait。
//! ```
//!
//! # 设计原则
//!
//! - **后端无关**：trait 中不出现执行引擎特化类型。
//! - **NaN-boxing 单一来源**：值编码常量与编解码函数来自 `wjsm-ir`，本 crate 复用。
//! - **零 vtable builtins**：`wjsm-builtins` 以 `<E: ExecContext>` 泛型实例化，编译期单态化。

mod call_args;
mod exec_context;
mod fetch_types;
mod heap_context;
mod json_value;
mod module_types;
pub mod property_key;
mod runtime_string;
mod stream_types;

pub use call_args::CallArgs;
pub use exec_context::{
    AtomicsRmwOp, BoundEntry, CapturedScope, ClosureEntry, ExecContext, IteratorNextStep,
    NativeCallableRef, PreparedCallback, PromiseCombinatorReactionKind, PromiseEntry,
    PromiseReaction, PromiseResolvingKind, PromiseSettlement, PromiseState, PropertyLookup,
    ProxyEntry, QueuingStrategySizeKind, ReactionType, RegExpMatchInfo, ToPrimitiveHintKind,
    TransformStreamFlushParams, TypedArrayView,
};
pub use fetch_types::{
    AbortSignalEntry, FetchRequestEntry, FetchResourceTimingState, FetchResponseEntry,
    HeadersEntry, HeadersGuard, HeadersMethodKind, HttpRequestSpec, RedirectMode, RequestCache,
    RequestCredentials, RequestMethodKind, RequestMode, ResponseMethodKind, ResponseType,
    SharedFetchResourceTiming,
};
pub use heap_context::{AsyncHookEvent, GcOutcome, HeapContext};
pub use json_value::JsonValue;
pub use module_types::{
    CjsRequireCacheTrapKind, RuntimeInstantiatedModule, RuntimeInstantiationEnv,
    RuntimeModuleFormat, RuntimeModuleImportResult, RuntimeModuleKey, RuntimeModuleLoadError,
    RuntimeModuleLoadErrorCode, RuntimeModuleReferrer, RuntimeModuleRequireResult,
    RuntimeModuleResolutionKind, RuntimeRequireCacheEntry, RuntimeResolvedModule,
};
pub use property_key::{
    DecodedNameId, decode_name_id, encode_runtime_string_name_id, encode_string_name_id,
    encode_symbol_name_id, is_symbol_name_id, name_id_to_property_key_value,
    symbol_value_to_name_id,
};
pub use runtime_string::{
    RuntimeString, code_point_at, content_hash_latin1, content_hash_units, ends_with_units,
    find_units, json_quote_units, rfind_units_before, starts_with_units,
};
pub use stream_types::{
    ByobRequestEntry, ControllerKind, ReadableStreamByobRequestMethodKind,
    ReadableStreamDefaultControllerMethodKind, ReadableStreamDefaultReaderMethodKind,
    ReadableStreamEntry, ReadableStreamMethodKind, ReadableStreamPipeToEntry, ReaderEntry,
    ReaderKind, StreamControllerEntry, StreamState, TransformStreamEntry,
    TransformStreamMethodKind, WritableStreamDefaultControllerMethodKind,
    WritableStreamDefaultWriterMethodKind, WritableStreamEntry, WritableStreamMethodKind,
    WritableStreamState, WriterEntry,
};

// ── 值与 handle：单一来源是 wjsm-ir 的 NaN-boxing 定义 ──
/// NaN-boxed JS 值（i64）。编码/解码见 `wjsm_ir::value`。
pub type Value = i64;
/// 对象 handle（obj_table 下标）。NaN-boxed 对象值的低 32 位。
pub type Handle = u32;
