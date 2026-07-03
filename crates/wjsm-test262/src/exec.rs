use std::{
    collections::HashMap,
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;

use rayon::prelude::*;

use crate::read::{Harness, Negative, Phase, Test, TestSuite};

/// 单条 Test262 用例的资源与超时限制（防止挂死 / WSL OOM）。
#[derive(Debug, Clone, Copy)]
pub struct RunLimits {
    /// 等待 `wjsm run` 子进程的最长时间；超时则 kill 并记为失败。
    pub timeout: Duration,
    /// Linux：子进程虚拟地址空间上限（MiB）；0 表示不设置。
    pub memory_limit_mib: u64,
    /// 并行 worker 数；建议 WSL 上保持 1–2。
    pub jobs: usize,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            memory_limit_mib: 512,
            jobs: 2,
        }
    }
}

/// 在 `timeout` 内等待子进程结束；超时返回 `Ok(None)`。
fn wait_child_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    let start = Instant::now();
    loop {
        match child.try_wait()? {
            Some(status) => return Ok(Some(status)),
            None if start.elapsed() >= timeout => return Ok(None),
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// 单个测试的结果。
#[derive(Debug, Clone)]
pub enum TestResult {
    Passed,
    Failed { expected: String, actual: String },
    Error(String),
}

/// 测试统计信息。
#[derive(Debug, Clone, Default)]
pub struct Statistics {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub ignored: usize,
    pub errors: usize,
}

impl Statistics {
    pub fn add(&mut self, result: &TestResult) {
        self.total += 1;
        match result {
            TestResult::Passed => self.passed += 1,
            TestResult::Failed { .. } => self.failed += 1,
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
    let wjsm_binary = find_wjsm_binary();

    // 模块模式测试不传 --script，使 wjsm 以 ES module 方式解析
    let is_module = test.is_module();

    let mut cmd = if wjsm_binary.file_name() == Some(std::ffi::OsStr::new("cargo")) {
        let mut c = Command::new("cargo");
        c.args(["run", "--bin", "wjsm-cli", "--", "run", "-"]);
        if !is_module {
            c.arg("--script");
        }
        c
    } else {
        let mut c = Command::new(&wjsm_binary);
        c.args(["run", "-"]);
        if !is_module {
            c.arg("--script");
        }
        c
    };

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "linux")]
    if limits.memory_limit_mib > 0 {
        let bytes = limits.memory_limit_mib.saturating_mul(1024 * 1024);
        unsafe {
            cmd.pre_exec(move || {
                let lim = libc::rlimit {
                    rlim_cur: bytes,
                    rlim_max: bytes,
                };
                if libc::setrlimit(libc::RLIMIT_AS, &lim) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return TestResult::Error(format!("failed to spawn wjsm: {}", e)),
    };

    if let Some(mut stdin) = child.stdin.take()
        && let Err(e) = stdin.write_all(source.as_bytes())
    {
        return TestResult::Error(format!("failed to write to stdin: {}", e));
    }

    let status = match wait_child_timeout(&mut child, limits.timeout) {
        Ok(Some(s)) => s,
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            return TestResult::Failed {
                expected: "complete within timeout".to_string(),
                actual: format!("timeout after {}s (child killed)", limits.timeout.as_secs()),
            };
        }
        Err(e) => return TestResult::Error(format!("failed to wait for wjsm: {}", e)),
    };

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = std::io::Read::read_to_end(&mut out, &mut stdout);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = std::io::Read::read_to_end(&mut err, &mut stderr);
    }

    evaluate_wjsm_output(test, &stdout, &stderr, status.code().unwrap_or(-1))
}

fn evaluate_wjsm_output(
    test: &Test,
    stdout_raw: &[u8],
    stderr_raw: &[u8],
    exit_code: i32,
) -> TestResult {
    let stdout = String::from_utf8_lossy(stdout_raw);
    let stderr = String::from_utf8_lossy(stderr_raw);

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

/// 查找 wjsm binary 路径。
fn find_wjsm_binary() -> std::path::PathBuf {
    // 优先使用 CARGO_BIN_EXE_wjsm 环境变量
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_wjsm") {
        return path.into();
    }

    // 尝试常见的构建输出路径
    let candidates = [
        "target/release/wjsm-cli",
        "target/debug/wjsm-cli",
        "target/release/wjsm",
        "target/debug/wjsm",
    ];

    for candidate in &candidates {
        let path = std::path::Path::new(candidate);
        if path.exists() {
            return path.to_path_buf();
        }
    }

    // 回退到 cargo run（会慢但有缓存时可用）
    std::path::PathBuf::from("cargo")
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
    if let TestResult::Failed { expected, actual } = result {
        failures.push(Failure {
            path: test.path.display().to_string(),
            expected: expected.clone(),
            actual: actual.clone(),
        });
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
}
