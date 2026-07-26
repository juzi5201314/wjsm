//! 后端无关的宿主环境（host environment）抽象。
//!
//! 本 crate 定义 JS runtime 的宿主能力 trait，**不依赖** wasmtime 或任何具体后端。
//! 各后端（wasmtime / native / cranelift）实现这些 trait 即可接入同一套 builtins 与
//! 动态代码服务（编译编排位于 `wjsm-host-wasm::compile_source`）。
//!
//! # 分层
//!
//! ```text
//! HostRuntime (组合 marker)
//!   ConsoleHost / ObjectHost / GcHost / AsyncHost   ← 纯语义 API，带 ctx
//!        │ 方法接收 &mut dyn HeapContext
//!        ▼
//! HeapContext                                       ← 堆/侧表最小操作集（解耦接缝）
//!        │
//! ExecContext: HeapContext                          ← builtins 完整能力（泛型单态化）
//!        │ 后端用自身运行时上下文实现
//!        ▼
//! wjsm-host-wasm: WasmExecContext  /  native 后端: NativeExecContext
//! ```
//!
//! # 设计原则
//!
//! - **后端无关**：trait 中不出现 `wasmtime::Caller` / `Store` 等后端特化类型。
//!   能力的语义实现经 [`HeapContext`] / [`ExecContext`] 落到后端堆/侧表。
//! - **NaN-boxing 单一来源**：值编码常量与编解码函数来自 `wjsm-ir`，本 crate 复用。
//! - **按需拆分**：`HostRuntime` 由多个 sub-trait 组合，便于后端按能力子集实现。
//! - **零 vtable builtins**：`wjsm-builtins` 以 `<E: ExecContext>` 泛型实例化，编译期单态化。

mod async_host;
mod console_host;
mod exec_context;
mod gc_host;
mod heap_context;
mod object_host;
pub mod property_key;
mod runtime_string;
mod runtime_trait;

pub use async_host::AsyncHost;
pub use console_host::ConsoleHost;
pub use exec_context::{
    BoundEntry, CapturedScope, ClosureEntry, ExecContext, ExecFuture, IteratorNextStep,
    NativeCallableRef, PromiseCombinatorReactionKind, PromiseEntry, PromiseReaction,
    PromiseResolvingKind, PromiseSettlement, PromiseState, ProxyEntry, QueuingStrategySizeKind,
    ReactionType, RegExpMatchInfo, TypedArrayView,
};
pub use gc_host::{GcHost, GcOutcome};
pub use heap_context::{AsyncHookEvent, HeapContext};
pub use object_host::ObjectHost;
pub use property_key::{
    DecodedNameId, decode_name_id, encode_runtime_string_name_id, encode_string_name_id,
    encode_symbol_name_id, is_symbol_name_id, name_id_to_property_key_value,
    symbol_value_to_name_id,
};
pub use runtime_string::RuntimeString;
pub use runtime_trait::HostRuntime;

// ── 值与 handle：单一来源是 wjsm-ir 的 NaN-boxing 定义 ──
/// NaN-boxed JS 值（i64）。编码/解码见 `wjsm_ir::value`。
pub type Value = i64;
/// 对象 handle（obj_table 下标）。NaN-boxed 对象值的低 32 位。
pub type Handle = u32;
