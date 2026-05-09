use std::{
    collections::HashMap,
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use rayon::prelude::*;

use crate::read::{Harness, Negative, Phase, Test, TestSuite};

/// 单个测试的结果。
#[derive(Debug, Clone)]
pub enum TestResult {
    Passed,
    Failed { expected: String, actual: String },
    Ignored,
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
            TestResult::Ignored => self.ignored += 1,
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

/// 运行单个测试。
pub fn run_test(test: &Test, harness: &Harness) -> TestResult {
    let source = build_test_source(test, harness);

    let wjsm_binary = find_wjsm_binary();

    let mut cmd = if wjsm_binary.file_name() == Some(std::ffi::OsStr::new("cargo")) {
        let mut c = Command::new("cargo");
        c.args(["run", "--bin", "wjsm", "--", "run", "-"]);
        c
    } else {
        let mut c = Command::new(&wjsm_binary);
        c.args(["run", "-"]);
        c
    };

    let mut child = match cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return TestResult::Error(format!("failed to spawn wjsm: {}", e)),
    };

    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(source.as_bytes()) {
            return TestResult::Error(format!("failed to write to stdin: {}", e));
        }
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return TestResult::Error(format!("failed to wait for wjsm: {}", e)),
    };

    let exit_code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // 判断是否预期失败
    if let Some(negative) = &test.metadata.negative {
        return check_negative_result(exit_code, &stderr, negative);
    }

    // 正常测试：预期通过（exit code 0）
    if exit_code == 0 {
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
    let candidates = ["target/debug/wjsm", "target/release/wjsm"];

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
fn check_negative_result(exit_code: i32, stderr: &str, negative: &Negative) -> TestResult {
    // wjsm 的 exit code 非零表示出错
    // 但需要匹配错误类型
    if exit_code == 0 {
        return TestResult::Failed {
            expected: format!("{} at {:?}", negative.error_type.as_str(), negative.phase),
            actual: "passed unexpectedly".to_string(),
        };
    }

    // 从 stderr 中检查错误类型是否匹配
    // 简单匹配：检查 stderr 中是否包含预期的错误类型名称
    let expected_type = negative.error_type.as_str();
    let stderr_lower = stderr.to_lowercase();

    // 根据 phase 判断
    match negative.phase {
        Phase::Parse | Phase::Early => {
            // 解析/早期错误通常在 stderr 中有 "SyntaxError" 或类似信息
            if stderr_lower.contains(&expected_type.to_lowercase()) {
                TestResult::Passed
            } else {
                TestResult::Failed {
                    expected: format!("{} error", expected_type),
                    actual: format!("stderr: {}", stderr.trim()),
                }
            }
        }
        Phase::Runtime | Phase::Resolution => {
            if stderr_lower.contains(&expected_type.to_lowercase()) {
                TestResult::Passed
            } else {
                TestResult::Failed {
                    expected: format!("{} error", expected_type),
                    actual: format!("stderr: {}", stderr.trim()),
                }
            }
        }
    }
}

fn build_test_source(test: &Test, harness: &Harness) -> String {
    let mut source = String::new();

    // 注入 wjsm 暂不支持的全局变量 workaround
    source.push_str("var undefined = void 0;\n");
    source.push_str("var NaN = 0 / 0;\n");
    source.push_str("var Infinity = 1 / 0;\n");
    source.push('\n');

    // raw 模式：只添加 workaround 和测试主体
    if test.is_raw() {
        if test.is_strict() {
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

    // 3. 添加 includes 中指定的文件
    for include in &test.metadata.includes {
        if let Some(file) = harness.includes.get(include) {
            source.push_str(&file.content);
            source.push('\n');
        }
    }

    // 4. 处理 flags
    if test.is_strict() {
        source.push_str("\"use strict\";\n");
    }

    // 5. 添加测试主体
    source.push_str(&test.source);
    source.push('\n');

    // 6. 添加 doneprintHandle
    source.push_str(&harness.doneprint_handle.content);
    source.push('\n');

    source
}

/// 运行整个测试套件。
pub fn run_suite(
    suite: &TestSuite,
    harness: &Harness,
    parallel: bool,
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

    if parallel {
        let results: Vec<(&Test, TestResult)> = tests
            .par_iter()
            .map(|test| (*test, run_test(test, harness)))
            .collect();

        for (test, result) in results {
            stats.add(&result);
            add_by_feature(&mut by_feature, test, &result);
            if let TestResult::Failed { expected, actual } = &result {
                failures.push(Failure {
                    path: test.path.display().to_string(),
                    expected: expected.clone(),
                    actual: actual.clone(),
                });
            }
        }
    } else {
        for test in tests {
            let result = run_test(test, harness);
            stats.add(&result);
            add_by_feature(&mut by_feature, test, &result);
            if let TestResult::Failed { expected, actual } = &result {
                failures.push(Failure {
                    path: test.path.display().to_string(),
                    expected: expected.clone(),
                    actual: actual.clone(),
                });
            }
        }
    }

    SuiteResults {
        stats,
        by_feature,
        failures,
        duration: start.elapsed(),
    }
}

fn add_by_feature(by_feature: &mut HashMap<String, Statistics>, test: &Test, result: &TestResult) {
    for feature in &test.metadata.features {
        by_feature.entry(feature.clone()).or_default().add(result);
    }
}
