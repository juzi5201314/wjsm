use anyhow::{Result, anyhow};
use clap::Parser;
use std::path::PathBuf;

/// 与 Node.js 的同机性能对比基准（hyperfine 驱动）。
#[derive(Debug, Parser)]
#[command(name = "wjsm-bench")]
pub struct Cli {
    /// 场景过滤：对 .js 文件名做子串匹配，默认全部。
    #[arg(long, default_value = "")]
    pub scenarios: String,
    /// 逗号分隔的运行时，默认 "node,wjsm"。
    #[arg(long, default_value = "node,wjsm")]
    pub runtimes: String,
    /// 追加 wjsm 冷启动档：快照关闭 + 每轮全新缓存目录。
    #[arg(long)]
    pub cold: bool,
    /// hyperfine 采样次数。
    #[arg(long, default_value_t = 10)]
    pub runs: usize,
    /// hyperfine 预热次数。
    #[arg(long, default_value_t = 3)]
    pub warmup: usize,
    /// --runs 3 --warmup 1 --window-ms 200 的冒烟快捷档（覆盖显式采样参数）。
    #[arg(long)]
    pub quick: bool,
    /// 场景内计时窗口毫秒（透传 BENCH_WINDOW_MS）。
    #[arg(long, default_value_t = 1000)]
    pub window_ms: u64,
    /// 场景内预热窗口毫秒（透传 BENCH_WARMUP_MS）。
    #[arg(long, default_value_t = 500)]
    pub warmup_ms: u64,
    /// JSON 报告输出路径；默认 /tmp/wjsm-bench-<unix秒>.json。
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// 打印每条 hyperfine 命令。
    #[arg(long, short = 'v')]
    pub verbose: bool,
}

/// `--quick` 展开后的采样配置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectiveConfig {
    pub runs: usize,
    pub warmup: usize,
    pub window_ms: u64,
    pub warmup_ms: u64,
}

impl Cli {
    /// `--quick` 优先于显式 --runs/--warmup/--window-ms。
    pub fn effective(&self) -> EffectiveConfig {
        if self.quick {
            EffectiveConfig {
                runs: 3,
                warmup: 1,
                window_ms: 200,
                warmup_ms: self.warmup_ms,
            }
        } else {
            EffectiveConfig {
                runs: self.runs,
                warmup: self.warmup,
                window_ms: self.window_ms,
                warmup_ms: self.warmup_ms,
            }
        }
    }
}

fn which(cmd: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(cmd);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

/// 解析 wjsm 可执行文件：`WJSM` env 优先，否则 PATH 中的 `wjsm`。
pub fn resolve_wjsm() -> Result<String> {
    if let Ok(value) = std::env::var("WJSM")
        && !value.trim().is_empty()
    {
        return Ok(value);
    }
    if let Some(found) = which("wjsm") {
        return Ok(found);
    }
    Err(anyhow!(
        "找不到 wjsm 可执行文件：请先 `cargo build --release -p wjsm-cli`，再用 WJSM=target/release/wjsm 指定"
    ))
}

/// 解析 node 可执行文件：`NODE` env 优先，否则 PATH 中的 `node`。
pub fn resolve_node() -> Result<String> {
    if let Ok(value) = std::env::var("NODE")
        && !value.trim().is_empty()
    {
        return Ok(value);
    }
    if let Some(found) = which("node") {
        return Ok(found);
    }
    Err(anyhow!(
        "找不到 node 可执行文件：请安装 Node.js，或用 NODE=<路径> 指定"
    ))
}
