use anyhow::Result;
use std::sync::Arc;

use crate::{
    GcExecutionStats, RuntimeOptions, compile_or_load_cached, run_init_globals_only,
    run_main_completion_block_async, run_startup_cold_path, runtime_engine_pool,
    runtime_source_map, runtime_startup, startup_snapshot_enabled, try_restore_snapshot,
};

/// 单个样本的输出、诊断与 GC 统计；`gc_stats.steady_state_ns` 只覆盖 user `main()`
/// 及其 JS microtask/scheduler 收尾，不包含 source compile、Wasm compile、instantiate 或 startup。
#[derive(Debug)]
pub struct SteadyStateExecution {
    pub output: Vec<u8>,
    pub diagnostics: Vec<u8>,
    pub gc_stats: GcExecutionStats,
}

/// 在测量边界外准备 engine/module/instance/startup，并只为 `main()` 记录 steady-state 时间。
///
/// benchmark 不支持 inspector，因为 inspector 启动会污染被测执行边界。
pub async fn execute_wasm_steady_state_for_bench(
    wasm_bytes: &[u8],
    options: RuntimeOptions,
) -> Result<SteadyStateExecution> {
    if options.inspect.is_some() {
        anyhow::bail!("steady-state benchmark does not support inspector options");
    }

    let key = runtime_engine_pool::engine_config_key(
        options.compiler,
        true,
        options.wasmtime_memory_reservation,
        false,
    );
    let pooled = runtime_engine_pool::acquire_engine(key)
        .map_err(|error| anyhow::anyhow!("create benchmark engine: {error:?}"))?;
    let engine = &pooled.engine;
    let epoch = Arc::clone(&pooled.epoch);
    let module = compile_or_load_cached(engine, wasm_bytes)?;
    let source_map = runtime_source_map::SourceMapInfo::parse_from_wasm(wasm_bytes);
    let snapshot_bytes = startup_snapshot_enabled()
        .then(|| crate::embedded_startup_snapshot_view(engine))
        .flatten();

    let mut bundle = runtime_startup::instantiate_execute_bundle_with_epoch(
        engine,
        &module,
        None,
        true,
        options.clone(),
        Some(Arc::clone(&epoch)),
    )
    .await?;
    bundle.store.data_mut().source_map = source_map.clone();

    let snapshot_restored = if let Some(bytes) = snapshot_bytes {
        let _ = run_init_globals_only(&mut bundle).await;
        try_restore_snapshot(&mut bundle, bytes).await
    } else {
        false
    };
    if !snapshot_restored {
        run_startup_cold_path(&mut bundle).await?;
    }

    let (output, diagnostics, gc_stats) = run_main_completion_block_async(
        &bundle.instance,
        bundle.store,
        bundle.wasm_env,
        bundle.output,
        bundle.runtime_error,
        bundle.diagnostics,
        Vec::new(),
        &mut bundle.host_completion_rx,
    )
    .await?;
    Ok(SteadyStateExecution {
        output,
        diagnostics,
        gc_stats,
    })
}
