//! Test262 runner 入口。
//!
//! 这个二进制用于批量执行 `test262/` 中的官方 ECMAScript conformance 用例。
//! 它不是 `cargo nextest` fixture runner：每条 Test262 用例都会构造完整测试源码，
//! 然后拉起一个独立的 `wjsm run -` 子进程执行。独立子进程是刻意设计：
//! 有 bug 的 JS 语义路径可能死循环、卡住 Promise/microtask、触发 runtime panic，
//! 不能让单条用例拖死整个 runner。
//!
//! ## 常用运行方式
//!
//! ```text
//! cargo build --bin wjsm
//! cargo run -p wjsm-test262 -- run --suite test/language/expressions/addition --plain -v
//! cargo run -p wjsm-test262 -- run --suite test/language/.../case.js --plain --no-parallel
//! cargo run -p wjsm-test262 -- run --suite test --json /tmp/test262.json
//! ```
//!
//! runner 默认只运行 `config.rs` 允许的 features；`--all` 会跳过该过滤，通常只用于
//! 探索覆盖率，不适合作为稳定 baseline。子进程默认从 `target/debug/wjsm` 等位置寻找
//! 二进制；如需指定其它构建产物，设置 `WJSM_TEST262_WJSM=/path/to/wjsm`。
//!
//! ## 超时模型
//!
//! 默认每条用例 `15s` 超时。超时后会 kill 子进程并计为 `TimedOut`，不会继续等待。
//! 这对 Test262 很重要：当某个测试覆盖到尚未实现或有 bug 的控制流 / async / iterator
//! 代码时，它可能永远不返回。可用 `--timeout-secs` 或 `WJSM_TEST262_TIMEOUT_SECS`
//! 调整；传 `0` 表示关闭超时，只有调试单条用例时才建议这么做。
//!
//! ## 内存与并发模型
//!
//! Wasmtime 43 在 64-bit 默认给 wasm32 linear memory 预留 `4GiB` 虚拟地址空间，
//! 另有 guard 区域和 growth reservation。它主要消耗虚拟地址空间，不等同于真实 RSS，
//! 但 Linux `RLIMIT_AS` 会按虚拟地址空间计数，因此不能直接把子进程地址空间上限
//! 当成并发物理内存成本。
//!
//! 这里把两个概念拆开：
//!
//! - `--memory-limit-mib` / `WJSM_TEST262_MEMORY_LIMIT_MIB`：Linux 子进程地址空间保护。
//!   默认 `5120MiB`，给 Rust runtime、Cranelift、线程栈、mmap 和 Wasmtime 预留留出余量。
//! - `--wasmtime-memory-reservation-mib` /
//!   `WJSM_TEST262_WASMTIME_MEMORY_RESERVATION_MIB`：传给 `wjsm` 的 Wasmtime 线性内存
//!   预留。默认 `256MiB`，runner 会通过 `--wasmtime-memory-reservation 256M` 降低
//!   Test262 子进程的虚拟地址压力。
//! - `--memory-budget-mib` / `WJSM_TEST262_MEMORY_BUDGET_MIB`：runner 允许并发 worker
//!   使用的近似物理内存预算。默认取宿主内存的一半，并限制在 `1GiB..32GiB`。
//! - `--job-memory-mib` / `WJSM_TEST262_JOB_MEMORY_MIB`：单个 worker 计入预算的估算
//!   物理内存成本，默认 `2048MiB`。实际并发为 `min(--jobs, budget / job_memory)`。
//! - `--jobs` / `WJSM_TEST262_JOBS`：期望并发数；不传时取可用 CPU 数，再按内存预算收敛。
//!
//! 例：`--jobs 32 --memory-budget-mib 8192 --job-memory-mib 2048` 最终会跑 `4` 个 worker。
//! 如果机器开始 swap 或 WSL 报 OOM，优先降低 `--memory-budget-mib`，或提高
//! `--job-memory-mib` 让同一预算下的并发更保守；如果子进程启动时报 `mmap failed` /
//! `memory allocation failed`，优先提高 `--memory-limit-mib`，而不是盲目降低并发。
//!
//! ## 相关实现边界
//!
//! - `main.rs`：CLI 参数、环境变量、默认资源策略、输出格式。
//! - `exec.rs`：构造测试源码、调度 suite、解释 Test262 pass/fail/negative/async 结果。
//! - `process.rs`：安全执行子进程、并发 drain stdin/stdout/stderr、超时 kill、输出截断。
//! - `read.rs`：读取 Test262 frontmatter、harness、suite 文件树。
//! - `config.rs`：当前 wjsm 已选择运行的 Test262 feature allowlist。

use std::{
    env,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueHint};
use colored::Colorize;
use comfy_table::{Table, presets::UTF8_HORIZONTAL_ONLY};

mod config;
mod exec;
mod process;
mod read;

use config::should_run_test;
use exec::{
    DEFAULT_CHILD_MEMORY_LIMIT_MIB, DEFAULT_JOB_MEMORY_COST_MIB, DEFAULT_TIMEOUT_SECS,
    DEFAULT_WASMTIME_MEMORY_RESERVATION_MIB, RunLimits, SuiteResults, TestResult,
};
use read::{read_harness, read_suite, read_test};

const TEST262_DIRECTORY: &str = "test262";

const ENV_TIMEOUT_SECS: &str = "WJSM_TEST262_TIMEOUT_SECS";
const ENV_CHILD_MEMORY_LIMIT_MIB: &str = "WJSM_TEST262_MEMORY_LIMIT_MIB";
const ENV_MEMORY_BUDGET_MIB: &str = "WJSM_TEST262_MEMORY_BUDGET_MIB";
const ENV_JOB_MEMORY_COST_MIB: &str = "WJSM_TEST262_JOB_MEMORY_MIB";
const ENV_WASMTIME_MEMORY_RESERVATION_MIB: &str = "WJSM_TEST262_WASMTIME_MEMORY_RESERVATION_MIB";
const ENV_JOBS: &str = "WJSM_TEST262_JOBS";
const DEFAULT_MEMORY_BUDGET_MAX_MIB: u64 = 32 * 1024;

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// 运行 Test262 测试套件。
    Run {
        /// 指定 Test262 子目录（相对 test262 目录的路径）。
        #[arg(short, long, default_value = "test", value_hint = ValueHint::AnyPath)]
        suite: PathBuf,

        /// 运行所有测试，不按 features 过滤。
        #[arg(long)]
        all: bool,

        /// 输出 JSON 结果到指定文件。
        #[arg(short, long, value_hint = ValueHint::FilePath)]
        json: Option<PathBuf>,

        /// 纯文本输出模式。
        #[arg(long)]
        plain: bool,

        /// 串行执行测试（推荐 WSL / 低内存环境）。
        #[arg(long)]
        no_parallel: bool,

        /// 单条用例超时（秒）；0 表示不设置超时。也可用 WJSM_TEST262_TIMEOUT_SECS。
        #[arg(long, value_name = "SECS")]
        timeout_secs: Option<u64>,

        /// Linux：单条子进程虚拟地址空间上限（MiB）；0 表示不限制。默认按 Wasmtime 预留量推导。
        #[arg(long, value_name = "MIB")]
        memory_limit_mib: Option<u64>,

        /// 单条用例的 Wasmtime 线性内存虚拟地址预留（MiB）；0 表示使用 Wasmtime 默认值。
        #[arg(long, value_name = "MIB")]
        wasmtime_memory_reservation_mib: Option<u64>,

        /// 整个 runner 的并发物理内存预算（MiB）；0 表示不按内存预算压低 jobs。也可用 WJSM_TEST262_MEMORY_BUDGET_MIB。
        #[arg(long, value_name = "MIB")]
        memory_budget_mib: Option<u64>,

        /// 单个并发 worker 计入预算的估算物理内存成本（MiB）。也可用 WJSM_TEST262_JOB_MEMORY_MIB。
        #[arg(long, value_name = "MIB")]
        job_memory_mib: Option<u64>,

        /// 并行 worker 数；默认按 CPU 自动选择，再按内存预算收敛。也可用 WJSM_TEST262_JOBS。
        #[arg(long, value_name = "N")]
        jobs: Option<usize>,

        /// 显示详细输出。
        #[arg(short, long, action = clap::ArgAction::Count)]
        verbose: u8,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Run {
            suite,
            all,
            json,
            plain,
            no_parallel,
            timeout_secs,
            memory_limit_mib,
            wasmtime_memory_reservation_mib,
            memory_budget_mib,
            job_memory_mib,
            jobs,
            verbose,
        } => {
            let limits = resolve_run_limits(
                no_parallel,
                timeout_secs,
                memory_limit_mib,
                wasmtime_memory_reservation_mib,
                memory_budget_mib,
                job_memory_mib,
                jobs,
            )?;
            run_test262(suite, all, json, plain, !no_parallel, limits, verbose)
        }
    }
}

fn resolve_run_limits(
    no_parallel: bool,
    timeout_secs: Option<u64>,
    memory_limit_mib: Option<u64>,
    wasmtime_memory_reservation_mib: Option<u64>,
    memory_budget_mib: Option<u64>,
    job_memory_mib: Option<u64>,
    jobs: Option<usize>,
) -> Result<RunLimits> {
    let timeout_secs = timeout_secs
        .or(parse_env_u64(ENV_TIMEOUT_SECS)?)
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    let wasmtime_memory_reservation_mib = wasmtime_memory_reservation_mib
        .or(parse_env_u64(ENV_WASMTIME_MEMORY_RESERVATION_MIB)?)
        .unwrap_or(DEFAULT_WASMTIME_MEMORY_RESERVATION_MIB);
    let child_memory_limit_mib = memory_limit_mib
        .or(parse_env_u64(ENV_CHILD_MEMORY_LIMIT_MIB)?)
        .unwrap_or_else(|| default_child_memory_limit_mib(wasmtime_memory_reservation_mib));
    let memory_budget_mib = memory_budget_mib
        .or(parse_env_u64(ENV_MEMORY_BUDGET_MIB)?)
        .unwrap_or_else(default_memory_budget_mib);
    let job_memory_cost_mib = job_memory_mib
        .or(parse_env_u64(ENV_JOB_MEMORY_COST_MIB)?)
        .unwrap_or(DEFAULT_JOB_MEMORY_COST_MIB);

    let requested_jobs = if no_parallel {
        1
    } else {
        jobs.or(parse_env_usize(ENV_JOBS)?)
            .unwrap_or_else(default_requested_jobs)
    };

    Ok(RunLimits {
        timeout: Duration::from_secs(timeout_secs),
        child_memory_limit_mib,
        wasmtime_memory_reservation_mib,
        memory_budget_mib,
        job_memory_cost_mib,
        jobs: if no_parallel {
            1
        } else {
            cap_jobs_by_memory_budget(requested_jobs, memory_budget_mib, job_memory_cost_mib)
        },
    })
}

fn cap_jobs_by_memory_budget(
    requested_jobs: usize,
    memory_budget_mib: u64,
    job_memory_cost_mib: u64,
) -> usize {
    let requested_jobs = requested_jobs.max(1);
    if memory_budget_mib == 0 {
        return requested_jobs;
    }

    let child_cost_mib = if job_memory_cost_mib == 0 {
        DEFAULT_JOB_MEMORY_COST_MIB
    } else {
        job_memory_cost_mib
    };
    let max_by_memory = (memory_budget_mib / child_cost_mib).max(1);
    let max_by_memory = usize::try_from(max_by_memory).unwrap_or(usize::MAX);
    requested_jobs.min(max_by_memory)
}

fn default_child_memory_limit_mib(wasmtime_memory_reservation_mib: u64) -> u64 {
    if wasmtime_memory_reservation_mib == 0 {
        8 * 1024
    } else {
        wasmtime_memory_reservation_mib
            .saturating_add(2 * 1024)
            .max(DEFAULT_CHILD_MEMORY_LIMIT_MIB)
    }
}

fn default_requested_jobs() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
}

fn default_memory_budget_mib() -> u64 {
    read_total_memory_mib()
        .map(|mib| (mib / 2).clamp(1024, DEFAULT_MEMORY_BUDGET_MAX_MIB))
        .unwrap_or(DEFAULT_CHILD_MEMORY_LIMIT_MIB)
}

#[cfg(target_os = "linux")]
fn read_total_memory_mib() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    meminfo.lines().find_map(|line| {
        let rest = line.strip_prefix("MemTotal:")?;
        let kb = rest.split_whitespace().next()?.parse::<u64>().ok()?;
        Some(kb / 1024)
    })
}

#[cfg(not(target_os = "linux"))]
fn read_total_memory_mib() -> Option<u64> {
    None
}

fn parse_env_u64(name: &str) -> Result<Option<u64>> {
    parse_env(name, str::parse::<u64>)
}

fn parse_env_usize(name: &str) -> Result<Option<usize>> {
    parse_env(name, str::parse::<usize>)
}

fn parse_env<T, E>(
    name: &str,
    parse: impl FnOnce(&str) -> std::result::Result<T, E>,
) -> Result<Option<T>>
where
    E: std::fmt::Display,
{
    match env::var(name) {
        Ok(value) => parse(&value)
            .map(Some)
            .map_err(|error| anyhow::anyhow!("invalid {name}={value}: {error}")),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(anyhow::anyhow!("could not read {name}: {error}")),
    }
}

fn run_test262(
    suite: PathBuf,
    all: bool,
    json_output: Option<PathBuf>,
    plain: bool,
    parallel: bool,
    limits: RunLimits,
    verbose: u8,
) -> Result<()> {
    let test262_path = Path::new(TEST262_DIRECTORY);

    if !test262_path.is_dir() {
        bail!(
            "test262 directory not found at {}. Please run `cargo build` first to clone it.",
            test262_path.display()
        );
    }

    if verbose > 0 {
        println!("Loading harness files...");
    }

    let harness = read_harness(test262_path).context("could not read test262 harness")?;

    let suite_path = test262_path.join(&suite);
    if !suite_path.exists() {
        bail!("suite path does not exist: {}", suite_path.display());
    }

    if suite_path.extension().and_then(|e| e.to_str()) == Some("js") {
        // 单个测试文件
        let test = read_test(&suite_path)
            .with_context(|| format!("could not read test: {}", suite_path.display()))?;

        if verbose > 0 {
            println!("Running single test: {}", suite_path.display());
        }

        let result = exec::run_test(&test, &harness, limits);
        print_single_result(&test, &result);
        return Ok(());
    }

    if verbose > 0 {
        println!("Reading test suite: {}", suite_path.display());
    }

    let suite = read_suite(&suite_path)
        .with_context(|| format!("could not read suite: {}", suite_path.display()))?;

    if verbose > 0 {
        println!("Running tests...");
        println!(
            "Limits: timeout={}s, wasmtime_memory_reservation_mib={}, child_memory_limit_mib={}, memory_budget_mib={}, job_memory_mib={}, jobs={}",
            limits.timeout.as_secs(),
            limits.wasmtime_memory_reservation_mib,
            limits.child_memory_limit_mib,
            limits.memory_budget_mib,
            limits.job_memory_cost_mib,
            limits.jobs
        );
    }

    let results = exec::run_suite(&suite, &harness, parallel, limits, &|test| {
        should_run_test(test, all)
    });

    // 输出结果
    if let Some(json_path) = json_output {
        write_json_results(&results, &json_path)?;
    } else if plain {
        print_plain_results(&results);
    } else {
        print_table_results(&results);
    }

    Ok(())
}

fn print_single_result(test: &read::Test, result: &TestResult) {
    match result {
        TestResult::Passed => {
            println!("{} {}", "PASS".green(), test.path.display());
        }
        TestResult::Failed { expected, actual } => {
            println!("{} {}", "FAIL".red(), test.path.display());
            println!("  expected: {}", expected);
            println!("  actual:   {}", actual);
        }
        TestResult::TimedOut { timeout, .. } => {
            println!("{} {}", "TIMEOUT".red().bold(), test.path.display());
            println!("  after: {}s", timeout.as_secs());
        }
        TestResult::Error(msg) => {
            println!("{} {}: {}", "ERROR".red().bold(), test.path.display(), msg);
        }
    }
}

fn print_plain_results(results: &SuiteResults) {
    let stats = &results.stats;
    println!();
    println!("Results:");
    println!("  Total:   {}", stats.total);
    println!("  Passed:  {}", stats.passed.to_string().green());
    println!("  Failed:  {}", stats.failed.to_string().red());
    println!("  Timeout: {}", stats.timed_out.to_string().red().bold());
    println!("  Ignored: {}", stats.ignored.to_string().yellow());
    println!("  Errors:  {}", stats.errors.to_string().red().bold());
    println!("  Rate:    {:.2}%", stats.conformance_rate());
    println!("  Time:    {:.2}s", results.duration.as_secs_f64());

    if !results.failures.is_empty() {
        println!();
        println!("Failures (first 10):");
        for failure in results.failures.iter().take(10) {
            println!("  - {}", failure.path);
        }
        if results.failures.len() > 10 {
            println!("  ... and {} more", results.failures.len() - 10);
        }
    }
}

fn print_table_results(results: &SuiteResults) {
    let stats = &results.stats;

    println!();
    let mut table = Table::new();
    table.load_preset(UTF8_HORIZONTAL_ONLY);
    table.set_header(vec!["Metric", "Value"]);
    table.add_row(vec!["Total Tests", &stats.total.to_string()]);
    table.add_row(vec![
        "Passed",
        &stats.passed.to_string().green().to_string(),
    ]);
    table.add_row(vec!["Failed", &stats.failed.to_string().red().to_string()]);
    table.add_row(vec![
        "Ignored",
        &stats.ignored.to_string().yellow().to_string(),
    ]);
    table.add_row(vec![
        "Timed Out",
        &stats.timed_out.to_string().red().bold().to_string(),
    ]);
    table.add_row(vec![
        "Errors",
        &stats.errors.to_string().red().bold().to_string(),
    ]);
    table.add_row(vec![
        "Conformance",
        &format!("{:.2}%", stats.conformance_rate()),
    ]);
    table.add_row(vec![
        "Duration",
        &format!("{:.2}s", results.duration.as_secs_f64()),
    ]);
    println!("{table}");

    // 按 feature 统计
    if !results.by_feature.is_empty() {
        println!();
        let mut table = Table::new();
        table.load_preset(UTF8_HORIZONTAL_ONLY);
        table.set_header(vec![
            "Feature", "Total", "Passed", "Failed", "Timeout", "Rate",
        ]);

        let mut features: Vec<_> = results.by_feature.iter().collect();
        features.sort_by_key(|(name, _)| *name);

        for (name, stats) in features {
            let rate = format!("{:.2}%", stats.conformance_rate());
            table.add_row(vec![
                name.as_str(),
                &stats.total.to_string(),
                &stats.passed.to_string(),
                &stats.failed.to_string(),
                &stats.timed_out.to_string(),
                &rate,
            ]);
        }

        println!("{table}");
    }

    if !results.failures.is_empty() {
        println!();
        println!("{} failures:", results.failures.len());
        for failure in results.failures.iter().take(20) {
            println!("  - {}", failure.path);
        }
        if results.failures.len() > 20 {
            println!("  ... and {} more", results.failures.len() - 20);
        }
    }
}

fn write_json_results(results: &SuiteResults, path: &Path) -> Result<()> {
    let json = serde_json::json!({
        "total": results.stats.total,
        "passed": results.stats.passed,
        "failed": results.stats.failed,
        "timed_out": results.stats.timed_out,
        "ignored": results.stats.ignored,
        "errors": results.stats.errors,
        "conformance_rate": results.stats.conformance_rate(),
        "duration_seconds": results.duration.as_secs_f64(),
        "by_feature": results.by_feature.iter().map(|(name, stats)| {
            (name.clone(), serde_json::json!({
                "total": stats.total,
                "passed": stats.passed,
                "failed": stats.failed,
                "ignored": stats.ignored,
                "timed_out": stats.timed_out,
                "errors": stats.errors,
            }))
        }).collect::<serde_json::Map<_, _>>(),
        "failures": results.failures.iter().map(|f| {
            serde_json::json!({
                "path": f.path,
                "expected": f.expected,
                "actual": f.actual,
            })
        }).collect::<Vec<_>>(),
    });

    std::fs::write(path, serde_json::to_string_pretty(&json)?)
        .with_context(|| format!("could not write JSON results to {}", path.display()))?;

    println!("Results written to {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_budget_caps_requested_jobs() {
        assert_eq!(
            cap_jobs_by_memory_budget(32, 16 * 1024, DEFAULT_JOB_MEMORY_COST_MIB),
            8
        );
    }

    #[test]
    fn zero_memory_budget_keeps_requested_jobs() {
        assert_eq!(
            cap_jobs_by_memory_budget(8, 0, DEFAULT_JOB_MEMORY_COST_MIB),
            8
        );
    }

    #[test]
    fn zero_job_memory_cost_uses_test262_default_cost_for_budget() {
        assert_eq!(cap_jobs_by_memory_budget(32, 16 * 1024, 0), 8);
    }
}
