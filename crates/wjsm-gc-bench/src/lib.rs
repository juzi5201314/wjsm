//! WJSM GC 轻量级性能基准。
//!
//! 只测量 WJSM 自身三种 collector 在固定 workload 下的性能指标，
//! 输出 JSON 供后续优化分析。不做跨引擎对比、不做 gate 判定。

pub mod cli;
pub mod report;
pub mod resource;
pub mod run;
pub mod runner;
pub mod scenario;
pub mod schema;
pub mod stats;
pub mod wjsm_driver;
