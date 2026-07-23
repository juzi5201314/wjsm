//! 碎片治理集成测试（issue #332）。
//!
//! 验证 non-moving mark-sweep 的尾部空间回收在真实 JS 执行下有效：
//! 长期 churn 后堆不膨胀失控，且存活对象正确。

use anyhow::Result;
use std::sync::LazyLock;
use tokio::runtime::Runtime;
use wjsm_runtime::{
    GcAlgorithmKind, RuntimeCompiler, RuntimeOptions, compile_source,
    execute_with_writer_with_options,
};

static RT: LazyLock<Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio")
});

fn run_js(source: &str) -> Result<String> {
    run_js_with_options(
        source,
        RuntimeOptions {
            compiler: Some(RuntimeCompiler::Cranelift),
            ..RuntimeOptions::default()
        },
    )
}

fn run_js_with_options(source: &str, options: RuntimeOptions) -> Result<String> {
    let wasm = compile_source(source)?;
    let mut opts = options;
    if opts.compiler.is_none() {
        opts.compiler = Some(RuntimeCompiler::Cranelift);
    }
    let (out, _) =
        RT.block_on(async { execute_with_writer_with_options(&wasm, Vec::new(), opts).await })?;
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

/// G1 mixed evacuation 正确性验证：在运行时单测中使用较短 churn，避免与
/// mark-sweep 长 churn 并发执行时触发 nextest 单测超时；完整 fixture 由
/// `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__gc_fragmentation_churn)'` 覆盖。
#[test]
fn g1_fragmentation_churn_survivors_intact() -> Result<()> {
    let output = run_js_with_options(
        r#"
        const survivor = { tag: "alive", data: [1, 2, 3, 4, 5] };
        for (let round = 0; round < 24; round++) {
            for (let i = 0; i < 64; i++) {
                const tmp = { a: i, b: i + 1, c: { round, i } };
                tmp.keep = survivor;
            }
        }
        gc();
        console.log(survivor.tag);
        console.log(survivor.data.join(","));
        console.log(survivor.data.reduce((a, b) => a + b, 0));
        "#,
        RuntimeOptions {
            gc_algorithm: GcAlgorithmKind::G1,
            ..RuntimeOptions::default()
        },
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

/// 数组扩容（V2 ensure_v2_array_capacity → release_region）后碎片回收验证。
/// 大量 grow 后不 OOM、不 corruption。
#[test]
fn array_grow_churn_no_oom() -> Result<()> {
    let output = run_js(
        r#"
        // 反复创建并扩容数组，触发 V2 ensure_v2_array_capacity → release_region 回收
        for (let round = 0; round < 100; round++) {
            let arr = [];
            for (let i = 0; i < 100; i++) {
                arr.push(i);
            }
            // arr 经历多次扩容，旧区由 V2 release_region 回收
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
