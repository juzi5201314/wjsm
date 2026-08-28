//! 宿主闭包三表与字符串 intern 表回收的回归测试（只断言表占用，不改 fixture）。
//!
//! 每个用例都用小堆迫使 GC，再读 `host_side_table_stats` 断言死槽被摘除、
//! 活槽存活。源码全部内联字符串，避免新建 fixture。

use std::path::Path;
use std::sync::Arc;

use wjsm_artifact_format::{ArtifactBuildInput, BuildOptions, ModuleManifest, PortableArtifact};

use super::*;

/// 表占用快照：`live_*` 只统计非空槽。
#[derive(Debug, Default)]
pub(crate) struct HostSideTableStats {
    pub live_closures: usize,
    pub function_closures: usize,
    pub latest_function_closures: usize,
    pub live_strings: usize,
    pub string_ids: usize,
    pub scope_records: usize,
}

fn artifact(source: &str) -> PortableArtifact {
    let source: Arc<str> = source.into();
    let ast = wjsm_parser::parse_module(&source).expect("source should parse");
    let program =
        wjsm_semantic::lower_module_with_source(ast, true, Some(Arc::clone(&source)), "input.js")
            .expect("source should lower");
    PortableArtifact::from_input(&ArtifactBuildInput {
        program: Arc::new(program),
        manifest: Arc::new(ModuleManifest::single("input.js", true)),
        options: BuildOptions::default(),
        source_text: None,
    })
    .expect("artifact should encode")
}

fn small_heap_runtime() -> NativeRuntime {
    let config = NativeRuntimeConfig::default().with_max_heap_size(4 * 1024 * 1024);
    NativeRuntime::new_with_config(config).expect("native runtime should initialize")
}

fn execute_source_with_runtime(runtime: &mut NativeRuntime, source: &str) -> NativeExecution {
    let artifact = artifact(source);
    runtime
        .execute(&artifact, Path::new("."), Path::new("."))
        .expect("source should execute")
}

/// 执行源码、强制 GC、返回表占用快照。
fn stats_after_gc(source: &str) -> HostSideTableStats {
    let mut runtime = small_heap_runtime();
    execute_source_with_runtime(&mut runtime, source);
    runtime.collect_garbage_now().expect("GC should run");
    runtime.host_side_table_stats()
}

#[test]
fn explicit_gc_waits_for_an_inflight_concurrent_cycle() {
    let mut runtime = small_heap_runtime();
    let artifact = artifact(
        "let keep={v:42}; for(let i=0;i<10;i++){keep.next={v:i,next:keep.next}} gc(); console.log(keep.v);",
    );
    let result = runtime.execute(&artifact, Path::new("."), Path::new("."));
    assert!(
        result.is_ok(),
        "result={result:?}, stderr={}",
        String::from_utf8_lossy(&runtime.take_stderr())
    );
    assert_eq!(result.unwrap().stdout, b"42\n");
}

/// issue #365：intern 路径只增不减 → 水位清扫。不调用显式 `gc()` 的唯一
/// 字符串 churn 循环里，`string_ids` 触水位即触发全量清扫；结束时表尺寸
/// 有界（远低于插入总量），且清扫/搬迁多轮后存活字符串内容不变。
#[test]
fn string_table_watermark_bounds_interned_strings_without_explicit_gc() {
    let mut runtime = small_heap_runtime();
    let execution = execute_source_with_runtime(
        &mut runtime,
        "const keep=['alive_'+123456789, 'bravo_'+987654321]; let n=0; \
         for(let i=0;i<60000;i++){ const s='xx'+i+'yy'; n+=s.length; } \
         console.log(n, keep.join('|'));",
    );
    assert_eq!(execution.stdout, b"528890 alive_123456789|bravo_987654321\n");
    let stats = runtime.host_side_table_stats();
    assert!(
        stats.string_ids < 16384,
        "水位清扫后 string_ids 应远低于 6 万插入量：{}",
        stats.string_ids
    );
    runtime.collect_garbage_now().expect("GC should run");
    let stats = runtime.host_side_table_stats();
    assert!(
        stats.string_ids < 4096,
        "全量 GC 后 string_ids 应回落到存活集：{}",
        stats.string_ids
    );
}

/// issue #365：regex match 文本/捕获组值是短命结果，走免入表路径，
/// `string_ids` 不随匹配数量增长（长 subject 字面量超过 64 码元去重上限，
/// 本身也不入表）。
#[test]
fn regexp_match_texts_do_not_enter_string_table() {
    let tokens = (0..40)
        .map(|i| format!("AA{}BB", 9_000_000 + i))
        .collect::<Vec<_>>()
        .join(" ");
    let source = format!(
        "const s='{tokens}'; const parts=s.match(/AA\\d+BB/g); \
         const m=/AA(\\d+)BB/.exec('ccAA12345678BBcc'); \
         console.log(parts.length, parts[0], parts[39], m[0], m[1], m.index);"
    );
    let mut runtime = small_heap_runtime();
    let before = runtime.host_side_table_stats().string_ids;
    let execution = execute_source_with_runtime(&mut runtime, &source);
    assert_eq!(
        execution.stdout,
        b"40 AA9000000BB AA9000039BB AA12345678BB 12345678 2\n"
    );
    let after = runtime.host_side_table_stats().string_ids;
    assert!(
        after <= before + 8,
        "40 个 match 文本 + exec 捕获组不应入表：before={before} after={after}"
    );
}

/// 回归：动态加法 lowering 的兄弟块（string 快路径 / dispatcher 慢路径）曾共用
/// `staged_dirty`，先 lower 的兄弟块清掉 dirty 后，慢路径带着陈旧 root frame 进
/// 宿主。intern 安全点此刻开启的并发 Young 标记看不到仅存于 SSA、正在构造的
/// 数组，随后的清扫把活数组误判为死（表现为 keep[0] 变成垃圾值或
/// InternalInvariant）。小堆下 pacing 恰好在 `'alive_'+N` 的 intern 处开启
/// Young 周期，确定性复现。
#[test]
fn array_under_construction_survives_mark_started_mid_expression() {
    let mut runtime = small_heap_runtime();
    let execution = execute_source_with_runtime(
        &mut runtime,
        "const keep=['alive_'+123456789]; gc(); console.log(keep[0]);",
    );
    assert_eq!(execution.stdout, b"alive_123456789\n");
}

#[test]
fn dead_closures_are_reclaimed() {
    let mut runtime = small_heap_runtime();
    let execution = execute_source_with_runtime(
        &mut runtime,
        "const add=(a)=>(b)=>a+b; let t=0; for(let i=0;i<5000;i++) t+=add(1)(2); console.log(t);",
    );
    assert_eq!(execution.stdout, b"15000\n");
    runtime.collect_garbage_now().expect("GC should run");
    let stats = runtime.host_side_table_stats();
    assert!(
        stats.live_closures < 64,
        "live_closures={}",
        stats.live_closures
    );
    assert!(
        stats.function_closures < 64,
        "function_closures={}",
        stats.function_closures
    );
    assert!(
        stats.latest_function_closures < 64,
        "latest_function_closures={}",
        stats.latest_function_closures
    );
}

#[test]
fn live_closure_survives_and_still_closes() {
    let mut runtime = small_heap_runtime();
    let execution = execute_source_with_runtime(
        &mut runtime,
        "const add=(a)=>(b)=>a+b; const live=add(40); let t=0; for(let i=0;i<5000;i++) t+=add(1)(2); globalThis.live=live; console.log(t, live(2));",
    );
    assert_eq!(execution.stdout, b"15000 42\n");
    runtime.collect_garbage_now().expect("GC should run");
    let stats = runtime.host_side_table_stats();
    assert!(
        stats.live_closures >= 1,
        "live_closures={}",
        stats.live_closures
    );
}

#[test]
fn interned_regexp_captures_are_reclaimed() {
    let mut runtime = small_heap_runtime();
    let execution = execute_source_with_runtime(
        &mut runtime,
        "const re=/a(\\d+)/; let n=0; for(let i=0;i<20000;i++){ const m=re.exec('a'+i); n+=m[1].length; } console.log(n);",
    );
    assert!(!execution.stdout.is_empty(), "regexp loop should print n");
    runtime.collect_garbage_now().expect("GC should run");
    let stats = runtime.host_side_table_stats();
    assert!(stats.string_ids < 4096, "string_ids={}", stats.string_ids);
    assert!(
        stats.live_strings < 4096,
        "live_strings={}",
        stats.live_strings
    );
}

#[test]
fn dead_closures_do_not_pin_script_scopes() {
    let baseline = stats_after_gc("console.log(1);").scope_records;
    let after =
        stats_after_gc("for(let i=0;i<200;i++){ eval('const add=(a)=>(b)=>a+b; add(1)(2);'); }")
            .scope_records;
    assert!(
        after <= baseline + 8,
        "scope_records {after} 不应明显大于启动快照基线 {baseline}"
    );
}

#[test]
fn reused_closure_slot_does_not_keep_old_environment() {
    let mut runtime = small_heap_runtime();
    let execution = execute_source_with_runtime(
        &mut runtime,
        "const live=(x)=>()=>x; const f=live(7); for(let i=0;i<20000;i++){ live(i)(); } console.log(f());",
    );
    assert_eq!(execution.stdout, b"7\n");
}
