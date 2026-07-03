//! 碎片治理集成测试（issue #332）。
//!
//! 验证 non-moving mark-sweep 的尾部空间回收在真实 JS 执行下有效：
//! 长期 churn 后堆不膨胀失控，且存活对象正确。

use anyhow::Result;
use tokio::runtime::Builder;
use wjsm_runtime::{compile_source, execute_with_writer};

fn run_js(source: &str) -> Result<String> {
    let wasm = compile_source(source)?;
    let rt = Builder::new_current_thread().enable_all().build()?;
    let (out, _) = rt.block_on(async { execute_with_writer(&wasm, Vec::new()).await })?;
    Ok(String::from_utf8(out)?)
}

/// 长期 churn 后正确性验证：大量分配-释放后存活对象完好。
#[test]
fn fragmentation_churn_survivors_intact() -> Result<()> {
    let output = run_js(
        r#"
        const survivor = { tag: "alive", data: [1, 2, 3, 4, 5] };
        for (let round = 0; round < 100; round++) {
            for (let i = 0; i < 200; i++) {
                const cap = (i % 10) + 1;
                const tmp = { a: i, b: i + 1 };
                for (let j = 0; j < cap; j++) {
                    tmp["k" + j] = j;
                }
            }
        }
        console.log(survivor.tag);
        console.log(survivor.data.join(","));
        console.log(survivor.data.reduce((a, b) => a + b, 0));
        "#,
    )?;
    assert_eq!(output, "alive\n1,2,3,4,5\n15\n");
    Ok(())
}

/// 交替大小对象分配后正确性验证（制造碎片后存活对象完好）。
#[test]
fn mixed_size_churn_preserves_state() -> Result<()> {
    let output = run_js(
        r#"
        const keep = [];
        for (let i = 0; i < 10; i++) {
            const obj = { id: i, value: i * 10 };
            keep.push(obj);
        }
        // 制造碎片：交替大小对象分配
        for (let round = 0; round < 50; round++) {
            const big = { data: round, extra: round };
            const small = { x: round };
        }
        // 验证 keep 中的对象
        let sum = 0;
        for (let j = 0; j < 10; j++) {
            sum += keep[j].id;
            sum += keep[j].value;
        }
        console.log(sum);
        "#,
    )?;
    // id sum = 0+1+...+9 = 45
    // value sum = 0+10+20+...+90 = 450
    // total = 45 + 450 = 495
    assert_eq!(output, "495\n");
    Ok(())
}

/// 数组扩容（grow_array → abandon 旧区域）后碎片回收验证。
/// 大量 grow 后不 OOM、不 corruption。
#[test]
fn array_grow_churn_no_oom() -> Result<()> {
    let output = run_js(
        r#"
        // 反复创建并扩容数组，触发 grow_array → abandon_region → sweep 回收
        for (let round = 0; round < 100; round++) {
            let arr = [];
            for (let i = 0; i < 100; i++) {
                arr.push(i);
            }
            // arr 经历多次扩容，旧区域被 abandon
            // 验证内容正确
            if (arr.length !== 100 || arr[50] !== 50) {
                console.log("FAIL");
                return;
            }
        }
        console.log("OK");
        "#,
    )?;
    assert_eq!(output, "OK\n");
    Ok(())
}
