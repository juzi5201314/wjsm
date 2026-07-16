//! 回归：async dispatch Switch 在 ≥3 个 resume case 时仍发射完整 br_if 与 suspend。

use std::fs;
use std::path::PathBuf;

use wjsm_backend_wasm::compile;
use wjsm_parser::parse_module;
use wjsm_semantic::lower_module;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/happy")
        .join(name)
}

#[test]
fn triple_await_async_switch_emits_three_dispatch_cases_and_suspends() {
    let source = fs::read_to_string(fixture_path("async_triple_await.js"))
        .expect("read async_triple_await.js");
    let ast = parse_module(&source).expect("parse");
    let program = lower_module(ast, false).expect("lower");
    let wasm = compile(&program).expect("compile");
    let wat = wasmprinter::print_bytes(&wasm).expect("wat");

    // state 0/1/2/3 的 f64 位模式（与 encode_constant(Number) 一致）
    for bits in [
        "0",
        "4607182418800017408",
        "4611686018427387904",
        "4613937818241073152",
    ] {
        assert!(
            wat.contains(&format!("i64.const {bits}")),
            "missing dispatch constant {bits} in WAT"
        );
    }

    let dispatch_eq = wat.matches("i64.eq").count();
    assert!(
        dispatch_eq >= 4,
        "expected at least 4 i64.eq for async switch, got {dispatch_eq}"
    );

    let suspend_markers =
        wat.matches("i64.const 3\n").count() + wat.matches("i64.const 3\r\n").count();
    assert!(
        suspend_markers >= 2,
        "expected multiple suspend state=3 sites in WAT, got {suspend_markers}"
    );
}
