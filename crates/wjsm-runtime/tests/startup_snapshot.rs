//! Startup snapshot on/off 一致性测试。
//!
//! 同一源码在 WJSM_STARTUP_SNAPSHOT=0（冷路径）和 =1（capture + restore）下
//! stdout/stderr/exit 必须完全一致。

use anyhow::Result;
use tokio::runtime::Runtime;
use wjsm_runtime::{compile_source, execute_with_writer};

fn run(source: &str) -> Result<String> {
    let wasm = compile_source(source)?;
    let rt = Runtime::new()?;
    let out = rt.block_on(async { execute_with_writer(&wasm, Vec::new()).await })?;
    Ok(String::from_utf8(out)?)
}

/// 涵盖 primordial 对象、数组方法、函数属性、GC 存活等 snapshot 敏感路径。
const FIXTURE: &str = r#"
const arr = [1, 2, 3];
console.log(arr.map(x => x * 2).join("-"));
console.log(arr.push(4), arr.length);
console.log(arr.reduce((s, x) => s + x, 0));

function f(x) { return x + 1; }
console.log(f.name, f(41));
Object.defineProperty(f, "custom", { value: 99, configurable: true });
console.log(f.custom);

const obj = { a: 1, b: 2 };
console.log(Object.keys(obj).join(","), JSON.stringify(obj));
"#;

#[test]
fn snapshot_off_produces_expected_output() -> Result<()> {
    // SAFETY: 单线程串行测试，无并发 env 访问
    unsafe { std::env::set_var("WJSM_STARTUP_SNAPSHOT", "0"); }
    let output = run(FIXTURE)?;
    unsafe { std::env::remove_var("WJSM_STARTUP_SNAPSHOT"); }

    assert_eq!(
        output,
        "2-4-6\n4 4\n10\nf 42\n99\na,b {\"a\":1,\"b\":2}\n"
    );
    Ok(())
}

#[test]
fn snapshot_on_off_same_output() -> Result<()> {
    // SAFETY: 单线程串行测试，无并发 env 访问
    unsafe { std::env::set_var("WJSM_STARTUP_SNAPSHOT", "0"); }
    let off_output = run(FIXTURE)?;

    // 开启 snapshot：第一次 capture（cold），第二次 restore（warm）
    unsafe { std::env::set_var("WJSM_STARTUP_SNAPSHOT", "1"); }
    let on_cold = run(FIXTURE)?;
    let on_warm = run(FIXTURE)?;
    unsafe { std::env::remove_var("WJSM_STARTUP_SNAPSHOT"); }

    assert_eq!(off_output, on_cold, "off vs on-cold mismatch");
    assert_eq!(off_output, on_warm, "off vs on-warm(restore) mismatch");
    Ok(())
}
