//! `wjsm-runtime`：direct native runtime 的稳定 facade。
//!
//! portable `.wjsm` artifact 由 `wjsm-artifact-format` 验证，当前宿主的 CLIF image
//! 与执行状态由 `wjsm-host-native` 唯一拥有。本 crate 只 re-export 公开 runtime API，
//! 不再承载编译器、字节码或异步执行桥。

pub use wjsm_gc::{GcAlgorithmKind, GcTelemetry, GcTelemetrySnapshot};
pub use wjsm_host_native::{
    NativeExecution, NativeRuntime, NativeRuntimeConfig, NativeRuntimeError, RuntimeInput,
    RuntimeOptions, SourceCompileOptions, compile_source, compile_source_with_options,
    execute_with_writer_with_options,
};
