//! `wjsm-runtime`：向后兼容 facade。
//!
//! 本 crate 是历史 `wjsm-runtime` 的**兼容外观**（facade）。自运行时按后端无关性
//! 拆分后，真正的实现迁到了以下 crate，本 crate 仅 re-export 它们的公开 API，
//! 保证现有代码（CLI / tests / 集成）零改动：
//!
//! - [`wjsm_host_wasm`]：wasmtime 执行引擎（builtins、解释器、RuntimeState、GC 接合点、
//!   `compile_source` 编译编排、engine config、support ABI、build-time 嵌入 artifact）
//! - [`wjsm_gc`]：后端无关 GC 算法（经 host-wasm re-export）
//! - [`wjsm_host`]：后端无关宿主能力 trait（多后端扩展点）
//!
//! # 拆分背景
//!
//! 见 `docs/adr/0011-runtime-split-by-backend-independence.md`。纯执行场景可只依赖
//! `wjsm-host-wasm`；开发新后端实现 `wjsm-host::HostRuntime`/`HeapContext`。

// 执行引擎 + GC + 编译编排的全部公开 API（含 RuntimeOptions、execute_*、
// compile_source、GC 类型、module loader、snapshot 等）。
pub use wjsm_host_wasm::*;

// 后端无关宿主能力 trait（多后端扩展点）。与 host-wasm 的 HostRuntime 实现并存。
pub use wjsm_host::{
    AsyncHookEvent, AsyncHost, ConsoleHost, GcHost, GcOutcome, Handle, HeapContext, HostRuntime,
    JsBackend, ObjectHost, Value,
};
