//! 此 crate 只通过 `wjsm-gc-bench` CLI 执行基准；内部测试全部标为 ignore，
//! 不参与 workspace 的常规正确性 gate。

pub mod cli;
pub mod comparison;
pub mod gate;
pub mod jdk_probe;
pub mod jvm_driver;
pub mod report;
pub mod resource;
pub mod run;
pub mod runner;
pub mod scenario;
pub mod schema;
pub mod stats;
pub mod wjsm_driver;

pub const EXIT_NEEDS_RESOURCE_RUNNER: i32 = 78;
