//! Property tests：每个测试只编译一次固定 runner，按 process.argv 传参执行。
//! 保留原三个测试名、ProptestConfig.cases=8、生成策略与逐字 expected。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, OnceLock};
use anyhow::{Context, Result, ensure};
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;
use tokio::runtime::Runtime;
use wjsm_runtime::{RuntimeCompiler, RuntimeOptions, compile_source, execute_with_writer_with_options};

/// 共享 current-thread Tokio runtime（整个 property 二进制复用）。
static PROP_RT: LazyLock<Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create shared property Tokio runtime")
});

fn run_wasm(wasm: &[u8], argv: Vec<String>) -> Result<String> {
    let options = RuntimeOptions {
        compiler: Some(RuntimeCompiler::Winch),
        argv,
        ..RuntimeOptions::default()
    };
    let (stdout, diagnostics) = PROP_RT
        .block_on(async { execute_with_writer_with_options(wasm, Vec::new(), options).await })
        .context("execute property source")?;
    ensure!(
        diagnostics.is_empty(),
        "runtime diagnostics: {}",
        String::from_utf8_lossy(&diagnostics)
    );
    String::from_utf8(stdout).context("stdout should be UTF-8")
}

fn prop_run_wasm(wasm: &[u8], argv: Vec<String>) -> Result<String, TestCaseError> {
    run_wasm(wasm, argv).map_err(|error| TestCaseError::fail(format!("{error:#}")))
}

fn base_argv(extra: &[String]) -> Vec<String> {
    let mut argv = vec!["wjsm".to_string(), "property.js".to_string()];
    argv.extend(extra.iter().cloned());
    argv
}

fn arithmetic_wasm() -> &'static [u8] {
    static WASM: OnceLock<Vec<u8>> = OnceLock::new();
    WASM.get_or_init(|| {
        // argv: [exe, script, a, b, d] — d 为除数（b==0 时为 1）
        compile_source(
            r#"
const a = Number(process.argv[2]);
const b = Number(process.argv[3]);
const d = Number(process.argv[4]);
console.log(a + b);
console.log(a - b);
console.log(a * b);
console.log(a % d);
"#,
        )
        .expect("compile arithmetic runner")
    })
    .as_slice()
}

fn string_wasm() -> &'static [u8] {
    static WASM: OnceLock<Vec<u8>> = OnceLock::new();
    WASM.get_or_init(|| {
        // argv: [exe, script, s]
        compile_source(
            r#"
const s = process.argv[2];
console.log(s.length);
console.log(s + "!");
"#,
        )
        .expect("compile string runner")
    })
    .as_slice()
}

fn coercion_wasm() -> &'static [u8] {
    static WASM: OnceLock<Vec<u8>> = OnceLock::new();
    WASM.get_or_init(|| {
        // argv: [exe, script, n, b, s] — b 为 "true"/"false"
        compile_source(
            r#"
const n = Number(process.argv[2]);
const b = process.argv[3] === "true";
const s = process.argv[4];
console.log(Number("  " + n + "  "));
console.log(String(n));
console.log(Boolean(n));
console.log(Number(b));
console.log(String(b));
console.log(Boolean(s));
"#,
        )
        .expect("compile coercion runner")
    })
    .as_slice()
}

/// 成功路径 case 计数器。
struct CaseCounter(AtomicUsize);
impl CaseCounter {
    const fn new() -> Self {
        Self(AtomicUsize::new(0))
    }
    fn tick(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn get(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

static ARITH_CASES: CaseCounter = CaseCounter::new();
static STRING_CASES: CaseCounter = CaseCounter::new();
static COERCE_CASES: CaseCounter = CaseCounter::new();

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 8,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn arithmetic_matches_integer_number_semantics(a in -1000i32..=1000, b in -1000i32..=1000) {
        let divisor = if b == 0 { 1 } else { b };
        let argv = base_argv(&[a.to_string(), b.to_string(), divisor.to_string()]);
        let expected = format!(
            "{}\n{}\n{}\n{}\n",
            a + b,
            a - b,
            a * b,
            a % divisor,
        );
        prop_assert_eq!(prop_run_wasm(arithmetic_wasm(), argv)?, expected);
        ARITH_CASES.tick();
    }

    #[test]
    fn string_length_and_concat_match_utf16_semantics(s in "[ -~]{0,32}") {
        let argv = base_argv(&[s.clone()]);
        let expected = format!("{}\n{}!\n", s.encode_utf16().count(), s);
        prop_assert_eq!(prop_run_wasm(string_wasm(), argv)?, expected);
        STRING_CASES.tick();
    }

    #[test]
    fn primitive_coercions_match_basic_ecmascript_rules(
        n in -1000i32..=1000,
        b in any::<bool>(),
        s in "[A-Za-z0-9 ]{0,16}",
    ) {
        let argv = base_argv(&[n.to_string(), b.to_string(), s.clone()]);
        let expected = format!(
            "{n}\n{n}\n{}\n{}\n{b}\n{}\n",
            n != 0,
            i32::from(b),
            !s.is_empty(),
        );
        prop_assert_eq!(prop_run_wasm(coercion_wasm(), argv)?, expected);
        COERCE_CASES.tick();
    }
}

/// argv 原样 roundtrip：引号、反斜杠、换行、非 BMP。
#[test]
fn argv_roundtrip_preserves_special_characters() {
    static WASM: OnceLock<Vec<u8>> = OnceLock::new();
    let wasm = WASM
        .get_or_init(|| {
            compile_source(r#"console.log(process.argv[2]);"#)
                .expect("compile argv roundtrip runner")
        })
        .as_slice();

    let cases = [
        r#"hello"world"#,
        r#"path\to\file"#,
        "line1\nline2",
        "emoji:😀",
        r#"mix \" \n 😀"#,
    ];
    for s in cases {
        let out = run_wasm(wasm, base_argv(&[s.to_string()])).expect("run argv roundtrip");
        assert_eq!(
            out.trim_end_matches('\n'),
            s,
            "argv roundtrip failed for {s:?}"
        );
    }
}

/// 同二进制内 proptest 成功路径应各执行 8 case。
/// nextest 默认每测一进程时计数器不跨测共享，因此此测只在同进程顺序执行时严格断言；
/// 单测进程下至少验证 runner 可执行。
#[test]
fn property_tests_execute_configured_case_count() {
    let a = ARITH_CASES.get();
    let s = STRING_CASES.get();
    let c = COERCE_CASES.get();
    if a > 0 || s > 0 || c > 0 {
        assert_eq!(a, 8, "arithmetic cases: {a}");
        assert_eq!(s, 8, "string cases: {s}");
        assert_eq!(c, 8, "coercion cases: {c}");
    }
    let out = run_wasm(
        arithmetic_wasm(),
        base_argv(&["3".into(), "4".into(), "4".into()]),
    )
    .expect("smoke arithmetic");
    assert_eq!(out, "7\n-1\n12\n3\n");
}
