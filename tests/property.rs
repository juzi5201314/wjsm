//! Property tests：每个 case 直接走 portable artifact → native image 执行路径。
//! 保留原三个语义性质、ProptestConfig.cases=8、生成策略与 expected。

use anyhow::{Context, Result, ensure};
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

#[path = "support/test_env.rs"]
mod test_env;

fn run_native(source: &str) -> Result<String> {
    test_env::ensure_test_cache_dir();
    let (exit_code, stdout, stderr) = wjsm_cli::run_source_in_process(source);
    ensure!(
        exit_code == 0,
        "native runtime failed: exit={exit_code}, stderr={}",
        String::from_utf8_lossy(&stderr)
    );
    ensure!(
        stderr.is_empty(),
        "native runtime diagnostics: {}",
        String::from_utf8_lossy(&stderr)
    );
    String::from_utf8(stdout).context("stdout should be UTF-8")
}

fn prop_run_native(source: &str) -> Result<String, TestCaseError> {
    run_native(source).map_err(|error| TestCaseError::fail(format!("{error:#}")))
}

fn js_string_literal(value: &str) -> String {
    let mut literal = String::with_capacity(value.len() + 2);
    literal.push('"');
    for character in value.chars() {
        match character {
            '\\' => literal.push_str("\\\\"),
            '"' => literal.push_str("\\\""),
            '\n' => literal.push_str("\\n"),
            '\r' => literal.push_str("\\r"),
            '\t' => literal.push_str("\\t"),
            character => literal.push(character),
        }
    }
    literal.push('"');
    literal
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 8,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn arithmetic_matches_integer_number_semantics(a in -1000i32..=1000, b in -1000i32..=1000) {
        let divisor = if b == 0 { 1 } else { b };
        let source = format!(
            "console.log({a} + {b}); console.log({a} - {b}); console.log({a} * {b}); console.log({a} % {divisor});",
        );
        let expected = format!(
            "{}\n{}\n{}\n{}\n",
            a + b,
            a - b,
            a * b,
            a % divisor,
        );
        prop_assert_eq!(prop_run_native(&source)?, expected);
    }

    #[test]
    fn string_length_and_concat_match_utf16_semantics(s in "[ -~]{0,32}") {
        let source = format!(
            "const s = {}; console.log(s.length); console.log(s + '!');",
            js_string_literal(&s),
        );
        let expected = format!("{}\n{}!\n", s.encode_utf16().count(), s);
        prop_assert_eq!(prop_run_native(&source)?, expected);
    }

    #[test]
    fn primitive_coercions_match_basic_ecmascript_rules(
        n in -1000i32..=1000,
        b in any::<bool>(),
        s in "[A-Za-z0-9 ]{0,16}",
    ) {
        let source = format!(
            "const n = {n}; const b = {b}; const s = {}; console.log(Number('  ' + n + '  ')); console.log(String(n)); console.log(Boolean(n)); console.log(Number(b)); console.log(String(b)); console.log(Boolean(s));",
            js_string_literal(&s),
        );
        let expected = format!(
            "{n}\n{n}\n{}\n{}\n{b}\n{}\n",
            n != 0,
            i32::from(b),
            !s.is_empty(),
        );
        prop_assert_eq!(prop_run_native(&source)?, expected);
    }
}

/// 字符串字面量转义保留引号、反斜杠、换行与非 BMP 字符。
#[test]
fn string_literal_roundtrip_preserves_special_characters() {
    let cases = [
        r#"hello"world"#,
        r#"path\to\file"#,
        "line1\nline2",
        "emoji:😀",
        r#"mix \" \n 😀"#,
    ];
    for value in cases {
        let source = format!("console.log({});", js_string_literal(value));
        let output = run_native(&source).expect("native string literal execution");
        assert_eq!(output.trim_end_matches('\n'), value);
    }
}
