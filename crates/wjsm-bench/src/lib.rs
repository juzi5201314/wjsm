//! wjsm × Node.js 跨运行时性能对比基准（hyperfine 驱动）。
//!
//! 同源码场景目录 + Rust harness，量化 wjsm 与 Node 在同机、固定版本、
//! 冷/稳态两档下的端到端与稳态性能差距，作为回归跟踪基线。

pub mod cli;
pub mod env;
pub mod report;
pub mod runner;
pub mod work_dir;

pub use cli::Cli;
pub use report::BenchReport;
pub use runner::run;
