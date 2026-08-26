//! 三类测量：wall（hyperfine）、ns_per_op（进程内稳态）、RSS（GNU time）。
//!
//! 所有子进程 `current_dir(repo_root)`；任何调用失败/非零退出都硬失败，
//! 错误信息含 场景/运行时/档位 + stderr 尾部。场景在 wjsm 下跑不通 = 必须修的缺陷。

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cli::{Cli, EffectiveConfig};
use crate::report::{
    BenchConfig, BenchReport, RegimeReport, RuntimeReport, ScenarioReport, WallStats, rfc3339_now,
    write_json,
};
use crate::work_dir::{cold_cache_dir, repo_root, scenarios_dir, work_dir};

/// 运行时种类，顺序即 hyperfine 命令顺序。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeKind {
    Node,
    Wjsm,
}

impl RuntimeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Wjsm => "wjsm",
        }
    }
}

/// 顶层入口：解析 → 采样 → 写报告 → 打印对比表。
pub fn run(cli: Cli) -> Result<i32> {
    let node_bin = crate::cli::resolve_node()?;
    let wjsm_bin = crate::cli::resolve_wjsm()?;
    let runtimes = parse_runtimes(&cli.runtimes)?;
    let scenarios = discover_scenarios(&cli.scenarios)?;
    let effective = cli.effective();
    let environment = crate::env::detect(&node_bin, &wjsm_bin);

    let mut regimes = BTreeMap::new();
    regimes.insert(
        "default".to_owned(),
        run_regime(
            &scenarios,
            &runtimes,
            &node_bin,
            &wjsm_bin,
            &effective,
            false,
            cli.verbose,
        )?,
    );
    if cli.cold {
        regimes.insert(
            "wjsm_cold".to_owned(),
            run_regime(
                &scenarios,
                &runtimes,
                &node_bin,
                &wjsm_bin,
                &effective,
                true,
                cli.verbose,
            )?,
        );
    }

    let report = BenchReport {
        schema_version: crate::report::BENCHMARK_SCHEMA_VERSION,
        created_at: rfc3339_now(),
        git_rev: environment.git_rev.clone(),
        config: BenchConfig {
            scenarios: scenarios.clone(),
            runtimes: runtimes.iter().map(|rt| rt.as_str().to_owned()).collect(),
            cold: cli.cold,
            runs: effective.runs,
            warmup: effective.warmup,
            window_ms: effective.window_ms,
            warmup_ms: cli.warmup_ms,
            iterations: effective.iterations,
        },
        environment,
        regimes,
    };

    let output = cli.output.clone().unwrap_or_else(default_output_path);
    write_json(&output, &report)?;
    print_table(&report);
    eprintln!("wjsm-bench: 报告 → {}", output.display());
    Ok(0)
}

fn parse_runtimes(runtimes: &str) -> Result<Vec<RuntimeKind>> {
    let mut out = Vec::new();
    for part in runtimes
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        match part {
            "node" => out.push(RuntimeKind::Node),
            "wjsm" => out.push(RuntimeKind::Wjsm),
            other => return Err(anyhow!("未知运行时 `{other}`（支持 node / wjsm）")),
        }
    }
    if out.is_empty() {
        return Err(anyhow!("--runtimes 不能为空（示例：node,wjsm）"));
    }
    Ok(out)
}

fn discover_scenarios(filter: &str) -> Result<Vec<String>> {
    let dir = scenarios_dir();
    let filters = filter
        .split(',')
        .map(str::trim)
        .filter(|filter| !filter.is_empty())
        .collect::<Vec<_>>();
    let mut names = Vec::new();
    for entry in
        std::fs::read_dir(&dir).with_context(|| format!("读取场景目录 {}", dir.display()))?
    {
        let entry = entry.context("读取场景目录项")?;
        let path = entry.path();
        if path.extension().map_or(false, |ext| ext == "js")
            && let Some(name) = path.file_stem().and_then(|stem| stem.to_str())
            && (filters.is_empty() || filters.iter().any(|filter| name.contains(filter)))
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    if names.is_empty() {
        return Err(anyhow!(
            "场景目录 {} 下没有匹配 `{}` 的 .js 场景",
            dir.display(),
            filter
        ));
    }
    Ok(names)
}

fn default_output_path() -> PathBuf {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    std::env::temp_dir()
        .join("wjsm-test-cache")
        .join("bench")
        .join(format!("wjsm-bench-{secs}.json"))
}

/// 采样单个档位：每场景 hyperfine wall + ns_per_op（仅 default）+ RSS（两档）。
fn run_regime(
    scenarios: &[String],
    runtimes: &[RuntimeKind],
    node_bin: &str,
    wjsm_bin: &str,
    effective: &EffectiveConfig,
    cold: bool,
    verbose: bool,
) -> Result<RegimeReport> {
    let mut regime = RegimeReport::default();
    for scenario in scenarios {
        let wall = measure_wall(
            scenario, runtimes, node_bin, wjsm_bin, effective, cold, verbose,
        )?;
        let mut scenario_report = ScenarioReport::default();
        for rt in runtimes {
            let mut runtime = RuntimeReport::default();
            if let Some(stats) = wall.get(rt.as_str()) {
                runtime.wall = Some(stats.clone());
            }
            if !cold {
                runtime.ns_per_op =
                    measure_ns_per_op(scenario, *rt, node_bin, wjsm_bin, effective)?;
            }
            runtime.max_rss_kb = measure_rss(scenario, *rt, node_bin, wjsm_bin, effective, cold)?;
            runtime.fixed_iter_rss_kb =
                measure_fixed_iter_rss(scenario, *rt, node_bin, wjsm_bin, effective, cold)?;
            match rt {
                RuntimeKind::Node => scenario_report.node = Some(runtime),
                RuntimeKind::Wjsm => scenario_report.wjsm = Some(runtime),
            }
        }
        regime.scenarios.insert(scenario.clone(), scenario_report);
    }
    Ok(regime)
}

fn regime_name(cold: bool) -> &'static str {
    if cold { "wjsm_cold" } else { "default" }
}

/// 对外部二进制做简单 shell 引用（hyperfine 命令走 shell）。
fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.@/%+=".contains(&byte))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn runtime_command(rt: RuntimeKind, node_bin: &str, wjsm_bin: &str, scenario: &str) -> String {
    let scenario_abs = scenarios_dir().join(format!("{scenario}.js"));
    match rt {
        RuntimeKind::Node => {
            format!(
                "{} {}",
                shell_quote(node_bin),
                shell_quote(&scenario_abs.to_string_lossy())
            )
        }
        RuntimeKind::Wjsm => format!(
            "{} run {}",
            shell_quote(wjsm_bin),
            shell_quote(&scenario_abs.to_string_lossy())
        ),
    }
}

/// wall 档：hyperfine 一次调用内对比所有选中运行时（interleave 公平）。
fn measure_wall(
    scenario: &str,
    runtimes: &[RuntimeKind],
    node_bin: &str,
    wjsm_bin: &str,
    effective: &EffectiveConfig,
    cold: bool,
    verbose: bool,
) -> Result<BTreeMap<String, WallStats>> {
    let json_path = work_dir()
        .join(regime_name(cold))
        .join(format!("{scenario}.json"));
    if let Some(parent) = json_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建 hyperfine 输出目录 {}", parent.display()))?;
    }

    let mut args = vec![
        "--warmup".to_owned(),
        effective.warmup.to_string(),
        "--runs".to_owned(),
        effective.runs.to_string(),
        "--style".to_owned(),
        "basic".to_owned(),
        "--export-json".to_owned(),
        json_path.display().to_string(),
    ];
    // cold 档：按命令顺序给每个运行时一条 --prepare。
    if cold {
        for rt in runtimes {
            let prepare = match rt {
                RuntimeKind::Node => "true".to_owned(),
                RuntimeKind::Wjsm => {
                    let dir = cold_cache_dir();
                    format!("rm -rf {} && mkdir -p {}", dir.display(), dir.display())
                }
            };
            args.push("-p".to_owned());
            args.push(prepare);
        }
    }
    let commands: Vec<(RuntimeKind, String)> = runtimes
        .iter()
        .map(|rt| (*rt, runtime_command(*rt, node_bin, wjsm_bin, scenario)))
        .collect();
    for (_, command) in &commands {
        args.push(format!("{command} > /dev/null"));
    }

    if verbose {
        eprintln!("$ hyperfine {}", args.join(" "));
    }

    let mut process = Command::new("hyperfine");
    process.args(&args).current_dir(repo_root());
    // bench 保持真实性能：禁用 IR 层 LICM，避免循环内纯 work() 被提升出循环
    // （fib30 场景提升后 ns_per_op 只剩空转开销）。hyperfine 命令行里的
    // `wjsm run ...` 是 shell 命令，环境变量设在 hyperfine 进程上即可透传；
    // node 忽略 WJSM_* 变量，无副作用。
    process.env("WJSM_DISABLE_LICM", "1");
    if cold {
        // 冷档只隔离 native / builtin 磁盘缓存；启动快照始终恢复。
        process.env("WJSM_CACHE_DIR", cold_cache_dir());
    }
    apply_scenario_env(&mut process, effective);
    let status = process.status().with_context(|| {
        format!(
            "hyperfine 执行失败（场景 {scenario}，档位 {}）",
            regime_name(cold)
        )
    })?;
    if !status.success() {
        return Err(anyhow!(
            "hyperfine 失败（场景 {scenario}，档位 {}），退出码 {status}",
            regime_name(cold)
        ));
    }
    parse_wall_json(&json_path, &commands)
}

/// 透传场景内计时窗口到子进程环境。
fn apply_scenario_env(process: &mut Command, effective: &EffectiveConfig) {
    process.env("BENCH_WINDOW_MS", effective.window_ms.to_string());
    process.env("BENCH_WARMUP_MS", effective.warmup_ms.to_string());
    process.env("BENCH_ITERATIONS", "0");
}

/// 透传固定迭代测量参数到子进程环境（关闭时间窗与预热）。
fn apply_fixed_iter_env(process: &mut Command, effective: &EffectiveConfig) {
    process.env("BENCH_ITERATIONS", effective.iterations.to_string());
    process.env("BENCH_WINDOW_MS", "0");
    process.env("BENCH_WARMUP_MS", "0");
}

#[derive(Deserialize)]
struct HyperfineReport {
    results: Vec<HyperfineResult>,
}

#[derive(Deserialize)]
struct HyperfineResult {
    command: String,
    mean: f64,
    stddev: f64,
    median: f64,
    min: f64,
    max: f64,
    times: Vec<f64>,
}

/// 解析 hyperfine `--export-json` 报告；结果顺序与命令顺序一一对应并交叉校验。
pub fn parse_wall_json(
    path: &Path,
    commands: &[(RuntimeKind, String)],
) -> Result<BTreeMap<String, WallStats>> {
    let payload =
        std::fs::read(path).with_context(|| format!("读取 hyperfine 报告 {}", path.display()))?;
    let parsed: HyperfineReport = serde_json::from_slice(&payload)
        .with_context(|| format!("解析 hyperfine 报告 {}", path.display()))?;
    if parsed.results.len() != commands.len() {
        return Err(anyhow!(
            "hyperfine 报告结果数 {} 与命令数 {} 不一致（{}）",
            parsed.results.len(),
            commands.len(),
            path.display()
        ));
    }
    let mut out = BTreeMap::new();
    for (index, (rt, command)) in commands.iter().enumerate() {
        let result = &parsed.results[index];
        // hyperfine 结果顺序即命令顺序；用 command 字段交叉校验，防止漂移。
        let expected = format!("{command} > /dev/null");
        if result.command != expected {
            return Err(anyhow!(
                "hyperfine 结果命令与预期不符（index {index}）：预期 `{expected}`，实际 `{}`",
                result.command
            ));
        }
        out.insert(
            rt.as_str().to_owned(),
            WallStats {
                mean_s: result.mean,
                stddev_s: result.stddev,
                median_s: result.median,
                min_s: result.min,
                max_s: result.max,
                runs: result.times.len(),
            },
        );
    }
    Ok(out)
}

/// ns_per_op：进程内稳态指标，仅 default 档采集。
fn measure_ns_per_op(
    scenario: &str,
    rt: RuntimeKind,
    node_bin: &str,
    wjsm_bin: &str,
    effective: &EffectiveConfig,
) -> Result<Option<f64>> {
    let (bin, args) = runtime_argv(rt, node_bin, wjsm_bin, scenario);
    let mut process = Command::new(bin);
    process.args(&args).current_dir(repo_root());
    // bench 保持真实性能：禁用 IR 层 LICM，避免循环内纯 work() 被提升出循环。
    if rt == RuntimeKind::Wjsm {
        process.env("WJSM_DISABLE_LICM", "1");
    }
    apply_scenario_env(&mut process, effective);
    let output = process
        .output()
        .with_context(|| format!("执行 {}（场景 {scenario}，ns_per_op）失败", rt.as_str()))?;
    if !output.status.success() {
        return Err(anyhow!(
            "{} 运行场景 {scenario} 失败（退出码 {}）：\n{}",
            rt.as_str(),
            output.status,
            tail_stderr(&output.stderr, 20)
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_ns_per_op(&stdout))
}

/// 各运行时的执行参数：node 直接跑脚本，wjsm 需要 `run` 子命令。
fn runtime_argv<'a>(
    rt: RuntimeKind,
    node_bin: &'a str,
    wjsm_bin: &'a str,
    scenario: &str,
) -> (&'a str, Vec<PathBuf>) {
    let scenario_abs = scenarios_dir().join(format!("{scenario}.js"));
    match rt {
        RuntimeKind::Node => (node_bin, vec![scenario_abs]),
        RuntimeKind::Wjsm => (wjsm_bin, vec![PathBuf::from("run"), scenario_abs]),
    }
}

/// 从 stdout 提取 `ns_per_op=<数值|Infinity>`；Infinity → None。
pub fn parse_ns_per_op(stdout: &str) -> Option<f64> {
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("ns_per_op=") {
            let token = rest.split_whitespace().next().unwrap_or_default();
            if token == "Infinity" {
                return None;
            }
            return token.parse::<f64>().ok();
        }
    }
    None
}

static RSS_HINT_PRINTED: AtomicBool = AtomicBool::new(false);

/// RSS 测量：`apply_env` 决定时间窗/固定迭代模式。
fn measure_rss_with_env(
    scenario: &str,
    rt: RuntimeKind,
    node_bin: &str,
    wjsm_bin: &str,
    effective: &EffectiveConfig,
    cold: bool,
    apply_env: impl Fn(&mut Command, &EffectiveConfig),
) -> Result<Option<u64>> {
    let available = cfg!(target_os = "linux") && Path::new("/usr/bin/time").exists();
    if !available {
        if !RSS_HINT_PRINTED.swap(true, Ordering::Relaxed) {
            eprintln!("wjsm-bench: 未找到 /usr/bin/time（GNU time），跳过 RSS 采集");
        }
        return Ok(None);
    }
    let (bin, args) = runtime_argv(rt, node_bin, wjsm_bin, scenario);
    let mut process = Command::new("/usr/bin/time");
    process
        .arg("-v")
        .arg(bin)
        .args(&args)
        .current_dir(repo_root())
        .stdout(Stdio::null());
    apply_env(&mut process, effective);
    if rt == RuntimeKind::Wjsm && cold {
        process.env("WJSM_CACHE_DIR", cold_cache_dir());
    }
    let output = process.output().with_context(|| {
        format!(
            "/usr/bin/time 执行失败（场景 {scenario}，运行时 {}）",
            rt.as_str()
        )
    })?;
    if !output.status.success() {
        return Err(anyhow!(
            "{} 运行场景 {scenario} 失败（退出码 {}）：\n{}",
            rt.as_str(),
            output.status,
            tail_stderr(&output.stderr, 20)
        ));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(parse_max_rss(&stderr))
}

/// 固定时间窗 RSS（BENCH_WINDOW_MS）。
fn measure_rss(
    scenario: &str,
    rt: RuntimeKind,
    node_bin: &str,
    wjsm_bin: &str,
    effective: &EffectiveConfig,
    cold: bool,
) -> Result<Option<u64>> {
    measure_rss_with_env(
        scenario,
        rt,
        node_bin,
        wjsm_bin,
        effective,
        cold,
        apply_scenario_env,
    )
}

/// 固定迭代 RSS（BENCH_ITERATIONS）。
fn measure_fixed_iter_rss(
    scenario: &str,
    rt: RuntimeKind,
    node_bin: &str,
    wjsm_bin: &str,
    effective: &EffectiveConfig,
    cold: bool,
) -> Result<Option<u64>> {
    measure_rss_with_env(
        scenario,
        rt,
        node_bin,
        wjsm_bin,
        effective,
        cold,
        apply_fixed_iter_env,
    )
}

/// 从 GNU time -v 的 stderr 提取最大驻留集。
pub fn parse_max_rss(stderr: &str) -> Option<u64> {
    for line in stderr.lines() {
        // GNU time 1.10 的 -v 输出每行以 `\t` 开头（od 验证过），先 trim 行首
        // 空白再 strip_prefix，否则匹配永远失败 → max_rss_kb 全 null。
        if let Some(rest) = line
            .trim_start()
            .strip_prefix("Maximum resident set size (kbytes): ")
        {
            return rest.trim().parse().ok();
        }
    }
    None
}

fn tail_stderr(stderr: &[u8], lines: usize) -> String {
    let lossy = String::from_utf8_lossy(stderr);
    let all: Vec<&str> = lossy.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

/// 终端对比表：纯 std 对齐。
fn print_table(report: &BenchReport) {
    let mut rows: Vec<Vec<String>> = vec![vec![
        "scenario".into(),
        "regime".into(),
        "wall median node ms".into(),
        "wall median wjsm ms".into(),
        "wjsm/node".into(),
        "ns_per_op node".into(),
        "ns_per_op wjsm".into(),
        "rss node KB".into(),
        "rss wjsm KB".into(),
        "iter rss node KB".into(),
        "iter rss wjsm KB".into(),
    ]];
    for (regime_name, regime) in &report.regimes {
        for (scenario, item) in &regime.scenarios {
            let node = item.node.as_ref();
            let wjsm = item.wjsm.as_ref();
            let node_wall_ms = node
                .and_then(|rt| rt.wall.as_ref())
                .map(|wall| wall.median_s * 1000.0);
            let wjsm_wall_ms = wjsm
                .and_then(|rt| rt.wall.as_ref())
                .map(|wall| wall.median_s * 1000.0);
            let ratio = match (wjsm_wall_ms, node_wall_ms) {
                (Some(wjsm_ms), Some(node_ms)) if node_ms > 0.0 => {
                    format!("{:.1}x", wjsm_ms / node_ms)
                }
                _ => "-".into(),
            };
            rows.push(vec![
                scenario.clone(),
                regime_name.clone(),
                fmt_f64(node_wall_ms, 2),
                fmt_f64(wjsm_wall_ms, 2),
                ratio,
                fmt_f64(node.and_then(|rt| rt.ns_per_op), 1),
                fmt_f64(wjsm.and_then(|rt| rt.ns_per_op), 1),
                fmt_u64(node.and_then(|rt| rt.max_rss_kb)),
                fmt_u64(wjsm.and_then(|rt| rt.max_rss_kb)),
                fmt_u64(node.and_then(|rt| rt.fixed_iter_rss_kb)),
                fmt_u64(wjsm.and_then(|rt| rt.fixed_iter_rss_kb)),
            ]);
        }
    }

    let widths: Vec<usize> = (0..rows[0].len())
        .map(|col| rows.iter().map(|row| row[col].len()).max().unwrap_or(0))
        .collect();
    let mut lines = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(col, cell)| {
                let width = widths[col];
                if index == 0 || col == 0 || col == 1 {
                    format!("{cell:<width$}")
                } else {
                    format!("{cell:>width$}")
                }
            })
            .collect();
        lines.push(cells.join(" | "));
    }
    println!("{}", lines.join("\n"));
}

fn fmt_f64(value: Option<f64>, precision: usize) -> String {
    match value {
        Some(value) => format!("{value:.precision$}"),
        None => "-".into(),
    }
}

fn fmt_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "-".into(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_max_rss_matches_gnu_time_tab_prefix() {
        // GNU time 1.10 的 -v 输出每行以 `\t` 开头，旧实现直接 strip_prefix 会失配。
        let stderr = "\tCommand being timed: \"wjsm run fib30.js\"\n\tMaximum resident set size (kbytes): 12345\n\tElapsed (wall clock) time (h:mm:ss or m:ss): 0:00.12\n";
        assert_eq!(parse_max_rss(stderr), Some(12345));
    }

    #[test]
    fn parse_max_rss_matches_plain_prefix() {
        // 无行首空白也应匹配（兼容其它 GNU time 版本）。
        let stderr = "Maximum resident set size (kbytes): 6789\n";
        assert_eq!(parse_max_rss(stderr), Some(6789));
    }

    #[test]
    fn parse_max_rss_missing_returns_none() {
        assert_eq!(parse_max_rss("no such line\n"), None);
    }
}
