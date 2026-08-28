//! `-e`/内联入口（无文件身份）的动态 import 集成测试：语义层把 referrer 发为
//! undefined（规范里 referrer 为 null），宿主必须提供默认解析基址并兑现命名
//! 空间；说明符 ToString 失败走 IfAbruptRejectPromise——全程不得 InternalInvariant。

use std::path::PathBuf;

fn run_inline(source: &str) -> (i32, String, String) {
    let (exit, stdout, stderr) = wjsm_cli::run_source_in_process(source);
    (
        exit,
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
    )
}

#[test]
fn inline_dynamic_import_builtin_fulfills_namespace() {
    let (exit, stdout, stderr) = run_inline(
        r#"import("node:url").then((ns) => console.log(typeof ns.fileURLToPath, typeof ns.URL));"#,
    );
    assert_eq!(exit, 0, "stderr: {stderr}");
    assert_eq!(stdout, "function function\n");
}

#[test]
fn inline_dynamic_import_relative_resolves_against_module_root() {
    // 显式 root 模拟从该目录执行 `run -e`：相对说明符以模块根为解析基址
    //（对齐 Node `--eval` 以 cwd 为基址）。
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/modules/dynamic_import_builtin_runtime");
    let (exit, stdout, stderr) = wjsm_cli::run_source_in_process_with_root(
        r#"import("./dep.mjs").then((ns) => console.log(ns.value));"#,
        &root,
    );
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = String::from_utf8_lossy(&stderr);
    assert_eq!(exit, 0, "stderr: {stderr}");
    assert_eq!(stdout, "42\n");
}

#[test]
fn inline_dynamic_import_missing_module_rejects() {
    let (exit, stdout, stderr) = run_inline(
        r#"import("./__wjsm_missing_module__.mjs").then(
  () => console.log("unexpected fulfill"),
  (error) => console.log("rejected", error instanceof Error),
);"#,
    );
    assert_eq!(exit, 0, "stderr: {stderr}");
    assert_eq!(stdout, "rejected true\n");
}

#[test]
fn inline_dynamic_import_specifier_tostring_rejects_symbol() {
    // EvaluateImportCall（§13.3.10.1.1）步骤 7-8：ToString(Symbol) 抛 TypeError，
    // promise 以该 TypeError 拒绝。
    let (exit, stdout, stderr) = run_inline(
        r#"import(Symbol()).then(
  () => console.log("unexpected fulfill"),
  (error) => console.log(error.name + ": " + error.message),
);"#,
    );
    assert_eq!(exit, 0, "stderr: {stderr}");
    assert_eq!(
        stdout,
        "TypeError: Cannot convert a Symbol value to a string\n"
    );
}

#[test]
fn inline_dynamic_import_specifier_coerces_via_tostring() {
    // 非字符串但可字符串化的 specifier 按 ToString 归一后再进入解析。
    let (exit, stdout, stderr) = run_inline(
        r#"const spec = { toString() { return "node:url"; } };
import(spec).then((ns) => console.log(typeof ns.pathToFileURL));"#,
    );
    assert_eq!(exit, 0, "stderr: {stderr}");
    assert_eq!(stdout, "function\n");
}
