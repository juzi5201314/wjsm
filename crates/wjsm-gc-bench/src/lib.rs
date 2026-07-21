//! 此 crate 只通过 `wjsm-gc-bench` CLI 执行基准；内部测试全部标为 ignore，
//! 不参与 workspace 的常规正确性 gate。
//!
//! 全部模块要求 `managed-heap-v2` feature（ADR 0010：性能证据只能产自 V2
//! ManagedHeap）。feature 关闭时本 crate 编译为空，从而 `--workspace` 构建
//! 不会把 V2 feature 统一进默认构建面。

#[cfg(feature = "managed-heap-v2")]
pub mod cli;
#[cfg(feature = "managed-heap-v2")]
pub mod comparison;
#[cfg(feature = "managed-heap-v2")]
pub mod gate;
#[cfg(feature = "managed-heap-v2")]
pub mod jdk_probe;
#[cfg(feature = "managed-heap-v2")]
pub mod jvm_driver;
#[cfg(feature = "managed-heap-v2")]
pub mod report;
#[cfg(feature = "managed-heap-v2")]
pub mod resource;
#[cfg(feature = "managed-heap-v2")]
pub mod run;
#[cfg(feature = "managed-heap-v2")]
pub mod runner;
#[cfg(feature = "managed-heap-v2")]
pub mod scenario;
#[cfg(feature = "managed-heap-v2")]
pub mod schema;
#[cfg(feature = "managed-heap-v2")]
pub mod stats;
#[cfg(feature = "managed-heap-v2")]
pub mod wjsm_driver;

pub const EXIT_NEEDS_RESOURCE_RUNNER: i32 = 78;
