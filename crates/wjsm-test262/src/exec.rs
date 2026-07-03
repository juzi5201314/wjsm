use std::{
    collections::HashMap,
    ffi::OsStr,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;

use rayon::prelude::*;

use crate::{
    process::{CapturedOutput, ProcessOutcome, run_with_input},
    read::{Harness, Negative, Phase, Test, TestSuite},
};

pub const DEFAULT_TIMEOUT_SECS: u64 = 15;
pub const DEFAULT_WASMTIME_MEMORY_RESERVATION_MIB: u64 = 256;
pub const DEFAULT_CHILD_MEMORY_LIMIT_MIB: u64 = 5 * 1024;
pub const DEFAULT_JOB_MEMORY_COST_MIB: u64 = 2 * 1024;

/// 单条 Test262 用例的资源与超时限制（防止挂死 / WSL OOM）。
#[derive(Debug, Clone, Copy)]
pub struct RunLimits {
    /// 等待 `wjsm run` 子进程的最长时间；0 表示不设置超时。
    pub timeout: Duration,
    /// Linux：单条子进程虚拟地址空间上限（MiB）；0 表示不设置。
    pub child_memory_limit_mib: u64,
    /// 传给 wjsm CLI 的 Wasmtime 线性内存虚拟地址预留（MiB）；0 表示使用 Wasmtime 默认值。
    pub wasmtime_memory_reservation_mib: u64,
    /// 整个 runner 允许并发子进程使用的近似物理内存预算（MiB）；0 表示不限制并发。
    pub memory_budget_mib: u64,
    /// 单个并发 worker 计入预算的估算物理内存成本（MiB）。
    pub job_memory_cost_mib: u64,
    /// 并行 worker 数；最终值应已按 `memory_budget_mib / job_memory_cost_mib` 收敛。
    pub jobs: usize,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            child_memory_limit_mib: DEFAULT_CHILD_MEMORY_LIMIT_MIB,
            wasmtime_memory_reservation_mib: DEFAULT_WASMTIME_MEMORY_RESERVATION_MIB,
            memory_budget_mib: DEFAULT_JOB_MEMORY_COST_MIB,
            job_memory_cost_mib: DEFAULT_JOB_MEMORY_COST_MIB,
            jobs: 1,
        }
    }
}

/// 单个测试的结果。
#[derive(Debug, Clone)]
pub enum TestResult {
    Passed,
    Failed {
        expected: String,
        actual: String,
    },
    TimedOut {
        timeout: Duration,
        stdout: String,
        stderr: String,
    },
    Error(String),
}

/// 测试统计信息。
#[derive(Debug, Clone, Default)]
pub struct Statistics {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub timed_out: usize,
    pub ignored: usize,
    pub errors: usize,
}

impl Statistics {
    pub fn add(&mut self, result: &TestResult) {
        self.total += 1;
        match result {
            TestResult::Passed => self.passed += 1,
            TestResult::Failed { .. } => self.failed += 1,
            TestResult::TimedOut { .. } => self.timed_out += 1,
            TestResult::Error(_) => self.errors += 1,
        }
    }

    pub fn conformance_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        (self.passed as f64 / self.total as f64) * 100.0
    }
}

/// 套件运行结果。
#[derive(Debug, Clone)]
pub struct SuiteResults {
    pub stats: Statistics,
    pub by_feature: HashMap<String, Statistics>,
    pub failures: Vec<Failure>,
    pub duration: Duration,
}

/// 失败记录。
#[derive(Debug, Clone)]
pub struct Failure {
    pub path: String,
    pub expected: String,
    pub actual: String,
}

/// 运行单个测试（带超时与可选内存上限）。
pub fn run_test(test: &Test, harness: &Harness, limits: RunLimits) -> TestResult {
    let source = build_test_source(test, harness);
    let mut command =
        match build_wjsm_command(test.is_module(), limits.wasmtime_memory_reservation_mib) {
            Ok(command) => command,
            Err(error) => return TestResult::Error(error),
        };

    apply_child_memory_limit(&mut command, limits.child_memory_limit_mib);

    match run_with_input(command, source.into_bytes(), limits.timeout) {
        Ok(ProcessOutcome::Completed(output)) => evaluate_wjsm_output(
            test,
            &output.stdout,
            &output.stderr,
            output.status.code().unwrap_or(-1),
        ),
        Ok(ProcessOutcome::TimedOut { stdout, stderr }) => TestResult::TimedOut {
            timeout: limits.timeout,
            stdout: stdout.text(),
            stderr: stderr.text(),
        },
        Err(error) => TestResult::Error(error),
    }
}

fn build_wjsm_command(
    is_module: bool,
    wasmtime_memory_reservation_mib: u64,
) -> Result<Command, String> {
    let wjsm_binary = find_wjsm_binary()?;
    let mut command = Command::new(&wjsm_binary);
    if wasmtime_memory_reservation_mib > 0 {
        command
            .arg("--wasmtime-memory-reservation")
            .arg(format!("{wasmtime_memory_reservation_mib}M"));
    }
    command.args(["run", "-"]);
    if !is_module {
        command.arg("--script");
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(command)
}

fn apply_child_memory_limit(command: &mut Command, memory_limit_mib: u64) {
    #[cfg(target_os = "linux")]
    if memory_limit_mib > 0 {
        let bytes = memory_limit_mib.saturating_mul(1024 * 1024);
        // SAFETY: `pre_exec` 闭包只调用 async-signal-safe 的 `setrlimit`，不分配内存、
        // 不获取锁，也不访问 Rust 共享状态；失败通过 `last_os_error` 返回给 `spawn`。
        unsafe {
            command.pre_exec(move || {
                let limit = libc::rlimit {
                    rlim_cur: bytes,
                    rlim_max: bytes,
                };
                if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    #[cfg(not(target_os = "linux"))]
    let _ = (command, memory_limit_mib);
}

fn find_wjsm_binary() -> Result<PathBuf, String> {
    if let Some(path) = binary_from_env("WJSM_TEST262_WJSM")? {
        return Ok(path);
    }
    if let Some(path) = binary_from_env("CARGO_BIN_EXE_wjsm")? {
        return Ok(path);
    }

    for candidate in wjsm_binary_candidates() {
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(
        "wjsm binary not found; run `cargo build --bin wjsm` first or set WJSM_TEST262_WJSM"
            .to_string(),
    )
}

fn binary_from_env(name: &str) -> Result<Option<PathBuf>, String> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    if path.exists() {
        Ok(Some(path))
    } else {
        Err(format!(
            "{name} points to missing binary: {}",
            path.display()
        ))
    }
}

fn wjsm_binary_candidates() -> [PathBuf; 4] {
    [
        executable_path("target/debug/wjsm"),
        executable_path("target/debug/wjsm-cli"),
        executable_path("target/release/wjsm"),
        executable_path("target/release/wjsm-cli"),
    ]
}

fn executable_path(path: &str) -> PathBuf {
    let mut path = PathBuf::from(path);
    if !std::env::consts::EXE_SUFFIX.is_empty() {
        let file_name = path
            .file_name()
            .unwrap_or_else(|| OsStr::new("wjsm"))
            .to_string_lossy();
        path.set_file_name(format!("{file_name}{}", std::env::consts::EXE_SUFFIX));
    }
    path
}

fn evaluate_wjsm_output(
    test: &Test,
    stdout_raw: &CapturedOutput,
    stderr_raw: &CapturedOutput,
    exit_code: i32,
) -> TestResult {
    let stdout = stdout_raw.text();
    let stderr = stderr_raw.text();

    if let Some(negative) = &test.metadata.negative {
        return check_negative_result(exit_code, &stderr, negative);
    }

    if test.is_async() {
        if stdout.contains("Test262:AsyncTestComplete") {
            TestResult::Passed
        } else if stdout.contains("Test262:AsyncTestFailure") {
            TestResult::Failed {
                expected: "Test262:AsyncTestComplete".to_string(),
                actual: format!("stdout={}", stdout.trim()),
            }
        } else {
            TestResult::Failed {
                expected: "Test262:AsyncTestComplete".to_string(),
                actual: format!(
                    "exit_code={}, stdout={}, stderr={}",
                    exit_code,
                    stdout.trim(),
                    stderr.trim()
                ),
            }
        }
    } else if exit_code == 0 {
        TestResult::Passed
    } else {
        TestResult::Failed {
            expected: "pass".to_string(),
            actual: format!("exit_code={}, stderr={}", exit_code, stderr.trim()),
        }
    }
}

/// 检查 negative 测试的结果。
///
/// wjsm 退出码约定：
/// - 0：成功
/// - 1：编译错误（parse/lower/compile），stderr 格式 `Error: {e:#}`
/// - 2：运行时错误（WASM 执行），stderr 格式 `Runtime error: {e:#}`
///
/// test262 phase 与 wjsm 阶段的对应：
/// - `Parse` / `Early`：编译期错误（exit 1）。wjsm 编译错误消息不含错误类型名
///   （如 "SyntaxError"），因此只检查退出码。
/// - `Resolution`：编译期或运行时（取决于实现，宽松匹配）。
/// - `Runtime`：运行时错误（exit 2）。wjsm 运行时错误消息含错误类型名
///   （如 "TypeError: ..."），因此额外检查 stderr 中是否包含预期类型。
fn check_negative_result(exit_code: i32, stderr: &str, negative: &Negative) -> TestResult {
    if exit_code == 0 {
        return TestResult::Failed {
            expected: format!("{} at {:?}", negative.error_type.as_str(), negative.phase),
            actual: "passed unexpectedly".to_string(),
        };
    }

    let expected_type = negative.error_type.as_str();

    // 退出码与 phase 匹配检查
    // exit 1 = 编译错误，exit 2 = 运行时错误
    let is_compile_error = exit_code == 1;
    let is_runtime_error = exit_code == 2;

    let phase_matches = match negative.phase {
        Phase::Parse | Phase::Early => is_compile_error,
        Phase::Resolution => is_compile_error || is_runtime_error,
        Phase::Runtime => is_runtime_error,
    };

    if !phase_matches {
        let expected_exit = match negative.phase {
            Phase::Parse | Phase::Early | Phase::Resolution => 1,
            Phase::Runtime => 2,
        };
        return TestResult::Failed {
            expected: format!(
                "{} at {:?} (expect exit {})",
                expected_type, negative.phase, expected_exit
            ),
            actual: format!("exit_code={}, stderr={}", exit_code, stderr.trim()),
        };
    }

    // 编译期错误（Parse/Early）：wjsm 错误消息格式为 `Error: error: <msg>`，
    // 不含错误类型名，因此只要退出码匹配即可通过。
    // Resolution 阶段同理——可能是编译期或运行时，退出码已匹配即可。
    if matches!(
        negative.phase,
        Phase::Parse | Phase::Early | Phase::Resolution
    ) {
        return TestResult::Passed;
    }

    // 运行时错误（Runtime）：wjsm 错误消息格式为 `Runtime error: <ErrorType>: <msg>`，
    // 检查 stderr 中是否包含预期的错误类型名称。
    let stderr_lower = stderr.to_lowercase();
    let expected_lower = expected_type.to_lowercase();

    if stderr_lower.contains(&expected_lower) {
        TestResult::Passed
    } else {
        TestResult::Failed {
            expected: format!("{} error at Runtime", expected_type),
            actual: format!("exit_code={}, stderr={}", exit_code, stderr.trim()),
        }
    }
}

fn build_test_source(test: &Test, harness: &Harness) -> String {
    let mut source = String::new();

    // 注入 wjsm 暂不支持的全局变量 workaround
    source.push_str("var undefined = void 0;\n");
    source.push_str("var NaN = 0 / 0;\n");
    source.push_str("var Infinity = 1 / 0;\n");
    // print() 是 test262 harness 使用的全局函数，wjsm 没有原生支持
    source.push_str("function print(msg) { console.log(msg); }\n");
    source.push('\n');
    // 设置 $262 对象（host-defined test262 API），保留运行时原生 gc 绑定。
    source.push_str("var $262 = { gc: gc };\n");
    source.push('\n');

    // raw 模式：只添加 workaround 和测试主体
    if test.is_raw() {
        // 模块模式隐含 strict，无需显式 "use strict"
        if test.is_strict() && !test.is_module() {
            source.push_str("\"use strict\";\n");
        }
        source.push_str(&test.source);
        source.push('\n');
        return source;
    }

    // 1. 添加 sta.js
    source.push_str(&harness.sta.content);
    source.push('\n');

    // 2. 添加 assert.js
    source.push_str(&harness.assert.content);
    source.push('\n');

    // 3. async 测试注入 asyncHelpers.js（提供 asyncTest 和 assert.throwsAsync）
    if test.is_async()
        && let Some(file) = harness.includes.get("asyncHelpers.js")
    {
        source.push_str(&file.content);
        source.push('\n');
    }

    // 4. 添加 includes 中指定的文件
    for include in &test.metadata.includes {
        if let Some(file) = harness.includes.get(include) {
            source.push_str(&file.content);
            source.push('\n');
        }
    }

    // 5. 处理 flags — 模块模式隐含 strict，无需显式 "use strict"
    if test.is_strict() && !test.is_module() {
        source.push_str("\"use strict\";\n");
    }

    // 6. 添加测试主体
    source.push_str(&test.source);
    source.push('\n');

    // 7. 添加 doneprintHandle
    source.push_str(&harness.doneprint_handle.content);
    source.push('\n');

    source
}

/// 运行整个测试套件。
pub fn run_suite(
    suite: &TestSuite,
    harness: &Harness,
    parallel: bool,
    limits: RunLimits,
    should_run: &dyn Fn(&Test) -> bool,
) -> SuiteResults {
    let start = Instant::now();
    let mut stats = Statistics::default();
    let mut by_feature: HashMap<String, Statistics> = HashMap::new();
    let mut failures = Vec::new();

    let tests: Vec<&Test> = crate::read::flatten_suite(suite)
        .into_iter()
        .filter(|t| should_run(t))
        .collect();

    if parallel && limits.jobs > 1 {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(limits.jobs)
            .build()
            .expect("rayon thread pool");
        let results: Vec<(&Test, TestResult)> = pool.install(|| {
            tests
                .par_iter()
                .map(|test| (*test, run_test(test, harness, limits)))
                .collect()
        });
        for (test, result) in results {
            record_result(&mut stats, &mut by_feature, &mut failures, test, &result);
        }
    } else {
        for test in &tests {
            let result = run_test(test, harness, limits);
            record_result(&mut stats, &mut by_feature, &mut failures, test, &result);
        }
    }

    SuiteResults {
        stats,
        by_feature,
        failures,
        duration: start.elapsed(),
    }
}

fn record_result(
    stats: &mut Statistics,
    by_feature: &mut HashMap<String, Statistics>,
    failures: &mut Vec<Failure>,
    test: &Test,
    result: &TestResult,
) {
    stats.add(result);
    add_by_feature(by_feature, test, result);
    if let Some((expected, actual)) = failure_details(result) {
        failures.push(Failure {
            path: test.path.display().to_string(),
            expected,
            actual,
        });
    }
}

fn failure_details(result: &TestResult) -> Option<(String, String)> {
    match result {
        TestResult::Failed { expected, actual } => Some((expected.clone(), actual.clone())),
        TestResult::TimedOut {
            timeout,
            stdout,
            stderr,
        } => Some((
            format!("complete within {}s", timeout.as_secs()),
            format!(
                "timeout after {}s; stdout={}; stderr={}",
                timeout.as_secs(),
                stdout.trim(),
                stderr.trim()
            ),
        )),
        TestResult::Passed | TestResult::Error(_) => None,
    }
}

fn add_by_feature(by_feature: &mut HashMap<String, Statistics>, test: &Test, result: &TestResult) {
    for feature in &test.metadata.features {
        by_feature.entry(feature.clone()).or_default().add(result);
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf};

    use super::*;
    use crate::read::{HarnessFile, MetaData};

    fn empty_harness() -> Harness {
        Harness {
            assert: HarnessFile {
                content: String::new(),
            },
            sta: HarnessFile {
                content: String::new(),
            },
            doneprint_handle: HarnessFile {
                content: String::new(),
            },
            includes: HashMap::new(),
        }
    }

    fn empty_test() -> Test {
        Test::new(
            PathBuf::from("gc-test.js"),
            MetaData {
                features: Vec::new(),
                includes: Vec::new(),
                flags: Vec::new(),
                negative: None,
            },
            String::new(),
        )
    }

    #[test]
    fn build_test_source_preserves_runtime_gc_binding() {
        let source = build_test_source(&empty_test(), &empty_harness());

        assert!(source.contains("var $262 = { gc: gc };"));
        assert!(!source.contains("function gc()"));
    }

    #[test]
    fn default_memory_settings_are_test262_friendly() {
        let limits = RunLimits::default();

        assert_eq!(
            limits.wasmtime_memory_reservation_mib,
            DEFAULT_WASMTIME_MEMORY_RESERVATION_MIB
        );
        assert_eq!(
            limits.child_memory_limit_mib,
            DEFAULT_CHILD_MEMORY_LIMIT_MIB
        );
        assert!(limits.child_memory_limit_mib > limits.wasmtime_memory_reservation_mib);
    }
}
