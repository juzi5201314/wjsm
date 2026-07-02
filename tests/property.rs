use anyhow::{Context, Result, ensure};
use proptest::prelude::*;
use tokio::runtime::Builder;
use wjsm_runtime::{compile_source, execute_with_writer};

fn run_js(source: &str) -> Result<String> {
    let wasm = compile_source(source).context("compile property source")?;
    let rt = Builder::new_current_thread()
        .enable_all()
        .build()
        .context("create runtime")?;
    let (stdout, diagnostics) = rt
        .block_on(async { execute_with_writer(&wasm, Vec::new()).await })
        .context("execute property source")?;
    ensure!(
        diagnostics.is_empty(),
        "runtime diagnostics: {}",
        String::from_utf8_lossy(&diagnostics)
    );
    String::from_utf8(stdout).context("stdout should be UTF-8")
}

fn prop_run_js(source: &str) -> Result<String, TestCaseError> {
    run_js(source).map_err(|error| TestCaseError::fail(format!("{error:#}")))
}

fn js_string_literal(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 2);
    out.push('"');
    for ch in input.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\u{08}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{0B}' => out.push_str("\\v"),
            '\u{0C}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            ch if ch.is_control() => out.push_str(&format!("\\u{:04X}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 8,
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    #[test]
    fn arithmetic_matches_integer_number_semantics(a in -1000i32..=1000, b in -1000i32..=1000) {
        let divisor = if b == 0 { 1 } else { b };
        let source = format!(
            "const a = {a}; const b = {b}; const d = {divisor};\n\
             console.log(a + b);\n\
             console.log(a - b);\n\
             console.log(a * b);\n\
             console.log(a % d);\n"
        );
        let expected = format!(
            "{}\n{}\n{}\n{}\n",
            a + b,
            a - b,
            a * b,
            a % divisor,
        );

        prop_assert_eq!(prop_run_js(&source)?, expected);
    }

    #[test]
    fn string_length_and_concat_match_utf16_semantics(s in "[ -~]{0,32}") {
        let literal = js_string_literal(&s);
        let source = format!(
            "const s = {literal};\n\
             console.log(s.length);\n\
             console.log(s + \"!\");\n"
        );
        let expected = format!("{}\n{}!\n", s.encode_utf16().count(), s);

        prop_assert_eq!(prop_run_js(&source)?, expected);
    }

    #[test]
    fn primitive_coercions_match_basic_ecmascript_rules(
        n in -1000i32..=1000,
        b in any::<bool>(),
        s in "[A-Za-z0-9 ]{0,16}",
    ) {
        let literal = js_string_literal(&s);
        let source = format!(
            "const n = {n}; const b = {b}; const s = {literal};\n\
             console.log(Number(\"  \" + n + \"  \"));\n\
             console.log(String(n));\n\
             console.log(Boolean(n));\n\
             console.log(Number(b));\n\
             console.log(String(b));\n\
             console.log(Boolean(s));\n"
        );
        let expected = format!(
            "{n}\n{n}\n{}\n{}\n{b}\n{}\n",
            n != 0,
            i32::from(b),
            !s.is_empty(),
        );

        prop_assert_eq!(prop_run_js(&source)?, expected);
    }
}
