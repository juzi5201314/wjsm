use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

use crate::scenario::ScenarioKind;

#[derive(Debug, Parser)]
#[command(name = "wjsm-gc-bench", about = "WJSM GC 轻量级性能基准")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 运行基准并输出 JSON 报告。
    Run(RunArgs),
    /// 输出主机资源与平台能力快照。
    Info(CommonArgs),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum GcKind {
    #[default]
    Zgc,
    G1,
    #[value(name = "mark-sweep")]
    MarkSweep,
}

impl GcKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Zgc => "zgc",
            Self::G1 => "g1",
            Self::MarkSweep => "mark-sweep",
        }
    }
}

#[derive(Clone, Debug, Parser)]
pub struct CommonArgs {
    /// 对象堆上限（如 32m、256m、1g）。
    #[arg(long, default_value = "256m", value_parser = parse_bytes)]
    pub heap: u64,
    /// JSON 输出路径。
    #[arg(long, default_value = "/tmp/wjsm-gc-bench.json")]
    pub output: PathBuf,
}

#[derive(Clone, Debug, Parser)]
pub struct RunArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// 选择 collector。
    #[arg(long, value_enum, default_value_t)]
    pub gc: GcKind,
    /// 存活集百分比（0–100）。
    #[arg(long, default_value_t = 50)]
    pub live_set: u8,
    /// workload 场景。
    #[arg(long, value_enum, default_value_t)]
    pub scenario: ScenarioKind,
    /// 重复采样次数。
    #[arg(long, default_value_t = 10)]
    pub samples: usize,
    /// 每个样本的 steady-state 持续秒数（0 = 单次执行）。
    #[arg(long, default_value_t = 0)]
    pub duration: u64,
    /// 随机种子。
    #[arg(long, default_value_t = 0x5eed)]
    pub seed: u64,
    /// 逻辑对象数（覆盖按堆大小自动计算的值）。
    #[arg(long)]
    pub objects: Option<u64>,
}

pub fn parse_bytes(input: &str) -> Result<u64, String> {
    let normalized = input.trim().to_ascii_lowercase();
    let (number, scale) = normalized
        .strip_suffix('g')
        .map(|number| (number, 1024_u64.pow(3)))
        .or_else(|| {
            normalized
                .strip_suffix('m')
                .map(|number| (number, 1024_u64.pow(2)))
        })
        .or_else(|| normalized.strip_suffix('k').map(|number| (number, 1024)))
        .unwrap_or((normalized.as_str(), 1));
    number
        .parse::<u64>()
        .map_err(|error| format!("invalid byte size `{input}`: {error}"))?
        .checked_mul(scale)
        .ok_or_else(|| format!("byte size `{input}` overflows u64"))
}
