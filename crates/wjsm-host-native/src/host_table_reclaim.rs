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

fn small_zgc_runtime() -> NativeRuntime {
    let config = NativeRuntimeConfig::default()
        .with_gc_algorithm(GcAlgorithmKind::Zgc)
        .with_max_heap_size(4 * 1024 * 1024);
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
    let mut runtime = small_zgc_runtime();
    execute_source_with_runtime(&mut runtime, source);
    runtime.collect_garbage_now().expect("GC should run");
    runtime.host_side_table_stats()
}

#[test]
fn dead_closures_are_reclaimed() {
    let mut runtime = small_zgc_runtime();
    let execution = execute_source_with_runtime(
        &mut runtime,
        "const add=(a)=>(b)=>a+b; let t=0; for(let i=0;i<20000;i++) t+=add(1)(2); console.log(t);",
    );
    assert_eq!(execution.stdout, b"60000\n");
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
    let mut runtime = small_zgc_runtime();
    let execution = execute_source_with_runtime(
        &mut runtime,
        "const add=(a)=>(b)=>a+b; const live=add(40); let t=0; for(let i=0;i<20000;i++) t+=add(1)(2); globalThis.live=live; console.log(t, live(2));",
    );
    assert_eq!(execution.stdout, b"60000 42\n");
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
    let mut runtime = small_zgc_runtime();
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
    let mut runtime = small_zgc_runtime();
    let execution = execute_source_with_runtime(
        &mut runtime,
        "const live=(x)=>()=>x; const f=live(7); for(let i=0;i<20000;i++){ live(i)(); } console.log(f());",
    );
    assert_eq!(execution.stdout, b"7\n");
}
