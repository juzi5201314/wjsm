use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Test262 测试文件的 YAML frontmatter 元数据。
#[derive(Debug, Clone, Deserialize)]
pub struct MetaData {
    pub description: String,
    pub esid: Option<String>,
    #[allow(dead_code)]
    pub es5id: Option<String>,
    pub es6id: Option<String>,
    #[serde(default)]
    pub info: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub includes: Vec<String>,
    #[serde(default)]
    pub flags: Vec<TestFlag>,
    #[serde(default)]
    pub negative: Option<Negative>,
}

/// 预期失败的测试信息。
#[derive(Debug, Clone, Deserialize)]
pub struct Negative {
    pub phase: Phase,
    #[serde(rename = "type")]
    pub error_type: ErrorType,
}

/// 错误发生的阶段。
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Parse,
    Early,
    Resolution,
    Runtime,
}

/// 错误类型。
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub enum ErrorType {
    Test262Error,
    SyntaxError,
    ReferenceError,
    RangeError,
    TypeError,
    EvalError,
}

impl ErrorType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Test262Error => "Test262Error",
            Self::SyntaxError => "SyntaxError",
            Self::ReferenceError => "ReferenceError",
            Self::RangeError => "RangeError",
            Self::TypeError => "TypeError",
            Self::EvalError => "EvalError",
        }
    }
}

/// 测试标记。
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TestFlag {
    OnlyStrict,
    NoStrict,
    Module,
    Raw,
    Async,
    Generated,
    #[serde(rename = "CanBlockIsFalse")]
    CanBlockIsFalse,
    #[serde(rename = "CanBlockIsTrue")]
    CanBlockIsTrue,
    #[serde(rename = "non-deterministic")]
    NonDeterministic,
}

/// 单个测试文件。
#[derive(Debug, Clone)]
pub struct Test {
    pub name: String,
    pub path: PathBuf,
    pub metadata: MetaData,
    pub source: String,
}

impl Test {
    pub fn new(name: String, path: PathBuf, metadata: MetaData, source: String) -> Self {
        Self {
            name,
            path,
            metadata,
            source,
        }
    }

    pub fn is_strict(&self) -> bool {
        self.metadata.flags.contains(&TestFlag::OnlyStrict)
    }

    pub fn is_module(&self) -> bool {
        self.metadata.flags.contains(&TestFlag::Module)
    }

    pub fn is_async(&self) -> bool {
        self.metadata.flags.contains(&TestFlag::Async)
    }

    pub fn is_raw(&self) -> bool {
        self.metadata.flags.contains(&TestFlag::Raw)
    }
}

/// Harness 辅助文件。
#[derive(Debug, Clone)]
pub struct HarnessFile {
    pub content: String,
    pub path: PathBuf,
}

/// 测试所需的 harness 文件集合。
#[derive(Debug, Clone)]
pub struct Harness {
    pub assert: HarnessFile,
    pub sta: HarnessFile,
    pub doneprint_handle: HarnessFile,
    pub includes: HashMap<String, HarnessFile>,
}

/// 测试套件（目录）。
#[derive(Debug, Clone)]
pub struct TestSuite {
    pub name: String,
    pub path: PathBuf,
    pub suites: Vec<TestSuite>,
    pub tests: Vec<Test>,
}

/// 读取 test262 的 harness 文件。
pub fn read_harness(test262_path: &Path) -> Result<Harness> {
    let harness_path = test262_path.join("harness");
    let mut includes = HashMap::new();

    read_harness_dir(&harness_path, &harness_path, &mut includes)?;

    let assert = includes
        .remove("assert.js")
        .context("failed to load harness file `assert.js`")?;
    let sta = includes
        .remove("sta.js")
        .context("failed to load harness file `sta.js`")?;
    let doneprint_handle = includes
        .remove("doneprintHandle.js")
        .context("failed to load harness file `doneprintHandle.js`")?;

    Ok(Harness {
        assert,
        sta,
        doneprint_handle,
        includes,
    })
}

fn read_harness_dir(
    harness_root: &Path,
    directory: &Path,
    includes: &mut HashMap<String, HarnessFile>,
) -> Result<()> {
    for entry in fs::read_dir(directory)
        .with_context(|| format!("error reading harness directory: {}", directory.display()))?
    {
        let entry = entry?;
        let path = entry.path();

        if entry.file_type()?.is_dir() {
            read_harness_dir(harness_root, &path, includes)?;
            continue;
        }

        let key = path
            .strip_prefix(harness_root)
            .with_context(|| format!("invalid harness file path: {}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");

        let content = fs::read_to_string(&path)
            .with_context(|| format!("error reading harness file: {}", path.display()))?;

        includes.insert(key, HarnessFile { content, path });
    }

    Ok(())
}

/// 递归读取测试套件。
pub fn read_suite(path: &Path) -> Result<TestSuite> {
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_string();

    let mut suites = Vec::new();
    let mut tests = Vec::new();

    for entry in
        fs::read_dir(path).with_context(|| format!("could not read suite: {}", path.display()))?
    {
        let entry = entry?;
        let path = entry.path();

        if entry.file_type()?.is_dir() {
            suites.push(read_suite(&path)?);
            continue;
        }

        if path.extension() != Some(OsStr::new("js")) {
            continue;
        }

        // 忽略 fixture 文件
        if path
            .file_stem()
            .is_some_and(|s| s.as_encoded_bytes().ends_with(b"FIXTURE"))
        {
            continue;
        }

        match read_test(&path) {
            Ok(test) => tests.push(test),
            Err(e) => {
                eprintln!("warning: failed to read test {}: {}", path.display(), e);
            }
        }
    }

    Ok(TestSuite {
        name,
        path: path.to_path_buf(),
        suites,
        tests,
    })
}

/// 读取单个测试文件。
pub fn read_test(path: &Path) -> Result<Test> {
    let name = path
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_string();

    let code = fs::read_to_string(path)
        .with_context(|| format!("could not read test file: {}", path.display()))?;

    let metadata = read_metadata(&code)?;

    Ok(Test::new(name, path.to_path_buf(), metadata, code))
}

/// 从测试代码中解析 YAML frontmatter。
fn read_metadata(code: &str) -> Result<MetaData> {
    let (_, metadata) = code
        .split_once("/*---")
        .ok_or_else(|| anyhow::anyhow!("invalid test metadata: missing /*---"))?;

    let (metadata, _) = metadata
        .split_once("---*/")
        .ok_or_else(|| anyhow::anyhow!("invalid test metadata: missing ---*/"))?;

    let metadata = metadata.replace('\r', "\n");

    serde_yaml::from_str(&metadata).with_context(|| "failed to parse YAML frontmatter")
}

/// 扁平化测试套件中的所有测试。
pub fn flatten_suite(suite: &TestSuite) -> Vec<&Test> {
    let mut tests = Vec::new();
    collect_tests(suite, &mut tests);
    tests
}

fn collect_tests<'a>(suite: &'a TestSuite, tests: &mut Vec<&'a Test>) {
    tests.extend(&suite.tests);
    for sub in &suite.suites {
        collect_tests(sub, tests);
    }
}
