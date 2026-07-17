use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

use crate::scenario::ScenarioKind;

#[derive(Debug, Parser)]
#[command(name = "wjsm-gc-bench", about = "可复现 WJSM GC 基准与归一化 gate")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Capabilities(CommonArgs),
    Preflight(CommonArgs),
    PrepareJdk(JdkArgs),
    Baseline(RunArgs),
    Run(RunArgs),
    Micro(MicroArgs),
    Compare(CompareArgs),
    Replay(ReplayArgs),
    Gate(GateArgs),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum EngineKind {
    #[default]
    Wjsm,
    Jdk,
}

impl EngineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wjsm => "wjsm",
            Self::Jdk => "jdk",
        }
    }
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum Profile {
    #[default]
    Pr,
    Nightly,
}

#[derive(Clone, Debug, Parser)]
pub struct CommonArgs {
    #[arg(long, default_value = "32m", value_parser = parse_bytes)]
    pub heap: u64,
    #[arg(long, default_value = "/tmp/wjsm-gc-bench.json")]
    pub output: PathBuf,
    #[arg(long, value_enum, default_value_t)]
    pub profile: Profile,
}

#[derive(Clone, Debug, Parser)]
pub struct JdkArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    #[arg(long)]
    pub jdk_home: Option<PathBuf>,
    #[arg(long)]
    pub jdk_probe_home: Option<PathBuf>,
}

#[derive(Clone, Debug, Parser)]
pub struct RunArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    #[arg(long, value_enum, default_value_t)]
    pub engine: EngineKind,
    #[arg(long, value_enum, default_value_t)]
    pub gc: GcKind,
    #[arg(long, default_value_t = 50)]
    pub live_set: u8,
    #[arg(long, value_enum, default_value_t)]
    pub scenario: ScenarioKind,
    #[arg(long, default_value_t = 30)]
    pub samples: usize,
    #[arg(long, default_value_t = 0)]
    pub duration: u64,
    #[arg(long, default_value_t = 1)]
    pub workers: usize,
    #[arg(long, default_value_t = 0x5eed)]
    pub seed: u64,
    #[arg(long)]
    pub manifest: Option<PathBuf>,
    #[arg(long)]
    pub jdk_home: Option<PathBuf>,
    #[arg(long)]
    pub jdk_probe_home: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    pub relocate_every_page: bool,
    #[arg(long, default_value_t = 4096)]
    pub barrier_buffer_capacity: usize,
    #[arg(long, default_value_t = false)]
    pub safepoint_every_allocation: bool,
}

#[derive(Clone, Debug, Parser)]
pub struct MicroArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    #[arg(long)]
    pub component: String,
    #[arg(long, default_value_t = 30)]
    pub samples: usize,
}

#[derive(Clone, Debug, Parser)]
pub struct CompareArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    #[arg(long)]
    pub jdk_home: PathBuf,
    #[arg(long)]
    pub jdk_probe_home: PathBuf,
    #[arg(long, default_value = "32m,256m,1024m")]
    pub heaps: String,
    #[arg(long, default_value = "10,50,80")]
    pub live_sets: String,
    #[arg(
        long,
        default_value = "churn,request,chain,cycle,wide,mutation,humongous,idle-uncommit"
    )]
    pub scenarios: String,
    #[arg(long, default_value_t = 30)]
    pub samples: usize,
}

#[derive(Debug, Parser)]
pub struct ReplayArgs {
    #[arg(long)]
    pub manifest: PathBuf,
    #[arg(long, default_value = "/tmp/wjsm-gc-replay.json")]
    pub output: PathBuf,
}

#[derive(Debug, Parser)]
pub struct GateArgs {
    #[arg(long)]
    pub manifest: PathBuf,
    #[arg(long, value_enum, default_value_t)]
    pub profile: Profile,
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
