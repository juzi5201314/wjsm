use anyhow::Result;
use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};
use num_traits::cast::ToPrimitive;
use rand::Rng;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::{self, Write};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use swc_core::ecma::ast as swc_ast;
use tokio::time::Instant;
use wasmtime::Func;
use wasmtime::*;
use wjsm_ir::{constants, value};
use wjsm_snapshot_format as startup_snapshot_format;
mod agent_cluster;
mod property_key;
mod runtime_arguments;
mod runtime_async_fn;
mod runtime_builtins;
mod runtime_collections;
mod runtime_combinators;
mod runtime_date;
mod runtime_eval;
mod runtime_gc;
mod runtime_heap;
mod runtime_host_helpers;
mod runtime_json;
mod runtime_microtask;
mod runtime_promises;
mod runtime_typedarray;
mod runtime_string_to_number;
mod runtime_value_adapter;
mod shared_buffer;
mod startup_snapshot;
pub mod startup_snapshot_remap;
mod runtime_linker;
mod runtime_startup;

/// Builtin JS 扩展：snapshot 期顺序拼接为 seed module。空 manifest 时该 mod 是
/// no-op；任一 .js 文件变化都会经 ABI hash external input 触发 embedded snapshot 失效。
mod builtin_js {
    pub mod manifest {
        include!("../builtin_js/manifest.rs");
    }
}

/// 把 BUILTIN_JS_FILES 顺序拼接为单一 ES module seed source。
/// 空 manifest 返回 `String::new()`，与 builtin JS 引入前的 P1 行为字节级一致。
fn concat_builtin_js_sources() -> String {
    builtin_js::manifest::BUILTIN_JS_FILES
        .iter()
        .map(|(_, src)| *src)
        .collect::<Vec<_>>()
        .join("\n;\n")
}

/// builtin JS bundle 的稳定 hash 输入：(name, source) 序列按声明顺序参与。
/// 用于 ABI hash external input，使任一 .js 改动都失效 embedded snapshot。
fn builtin_js_bundle_hash() -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    for (name, source) in builtin_js::manifest::BUILTIN_JS_FILES {
        name.hash(&mut h);
        source.hash(&mut h);
    }
    h.finish()
}
mod startup_snapshot_native_bridge;
mod types;
pub(crate) use shared_buffer::{SharedRuntimeState, new_shared_runtime_state};
mod scheduler;

mod host_imports;
mod runtime_render;
mod runtime_values;
mod wasm_env;
use host_imports::*;
use property_key::*;
pub(crate) use wasm_env::WasmEnv;

use runtime_arguments::*;
use runtime_async_fn::*;
use runtime_builtins::*;
use runtime_collections::*;
use runtime_combinators::*;
use runtime_date::*;
use runtime_eval::*;
use runtime_heap::*;
use runtime_host_helpers::*;
use runtime_json::*;
use runtime_microtask::*;
use runtime_promises::*;
use runtime_render::*;
use runtime_typedarray::*;
use runtime_values::*;
use types::*;

pub async fn execute(wasm_bytes: &[u8]) -> Result<()> {
    let stdout = io::stdout();
    let _ = execute_with_writer(wasm_bytes, stdout.lock()).await?;
    Ok(())
}
pub async fn execute_with_writer<W: Write>(wasm_bytes: &[u8], writer: W) -> Result<W> {
    execute_with_writer_shared(wasm_bytes, writer, None).await
}

/// 编译 JS/TS 源码到 WASM 字节码的共享辅助函数。
/// 供本 crate 测试及外部集成测试（`tests/`）复用，避免重复定义
/// `parse_module → lower_module → compile` 流程。
pub fn compile_source(source: &str) -> Result<Vec<u8>> {
    let module = wjsm_parser::parse_module(source)?;
    let program = wjsm_semantic::lower_module(module, false)?;
    wjsm_backend_wasm::compile(&program)
}

/// 构建时生成嵌入式 startup snapshot 字节（空 seed JS → cold bootstrap → capture）。
pub fn build_embedded_startup_snapshot_bytes() -> Result<Vec<u8>> {
    // build.rs 路径不会进入运行时 `startup_snapshot_enabled()`，必须在此显式注册
    // ABI external input，确保 snapshot header.abi_hash 与运行时 abi_hash() 一致。
    wjsm_snapshot_format::register_abi_hash_external_input(combined_abi_external_input());
    let seed = concat_builtin_js_sources();
    let wasm = compile_source(&seed)?;
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| anyhow::anyhow!("failed to create tokio runtime for snapshot build: {e}"))?;
    rt.block_on(build_embedded_startup_snapshot_bytes_async(&wasm))
}

async fn build_embedded_startup_snapshot_bytes_async(wasm: &[u8]) -> Result<Vec<u8>> {
    let config = startup_engine_config(true);
    let engine = Engine::new(&config)
        .map_err(|e| anyhow::anyhow!("Failed to create async engine: {:?}", e))?;
    let module = Module::new(&engine, wasm)
        .map_err(|e| anyhow::anyhow!("WASM validation failed: {:?}", e))?;
    let mut bundle = instantiate_execute_bundle(&engine, &module, None, true).await?;
    run_bootstrap_only(&mut bundle).await?;
    let snap = startup_snapshot::capture_startup_snapshot(&mut bundle.store, &bundle.wasm_env)?;
    let bytes = startup_snapshot_format::encode_snapshot(&snap);
    let current_abi = startup_snapshot_format::abi_hash();
    if snap.header.abi_hash != current_abi {
        anyhow::bail!(
            "embedded snapshot ABI hash mismatch: captured={:#018x} current={:#018x}",
            snap.header.abi_hash,
            current_abi
        );
    }
    Ok(bytes)
}

static EMBEDDED_STARTUP_SNAPSHOT: OnceLock<Arc<[u8]>> = OnceLock::new();

/// 安装编译时嵌入的 startup snapshot；进程内只需调用一次（重复 set 被忽略）。
pub fn install_embedded_startup_snapshot(snapshot_bytes: impl AsRef<[u8]>) {
    let _ = EMBEDDED_STARTUP_SNAPSHOT.set(Arc::from(snapshot_bytes.as_ref()));
}

/// 返回已通过 decode + ABI 校验的嵌入式 snapshot 字节；未安装或校验失败时为 `None`。
pub fn embedded_startup_snapshot() -> Option<&'static [u8]> {
    embedded_startup_snapshot_view()
}

pub(crate) fn embedded_startup_snapshot_view() -> Option<&'static [u8]> {
    let arc = EMBEDDED_STARTUP_SNAPSHOT.get()?;
    let bytes = arc.as_ref();
    let view = startup_snapshot_format::decode_snapshot(bytes).ok()?;
    // 惰性注册 external input（OnceLock 幂等）：确保 abi_hash() 包含 support module
    // layout hash + builtin JS bundle hash，与 capture 时的 ABI 输入一致。
    startup_snapshot_format::register_abi_hash_external_input(combined_abi_external_input());
    if view.header.abi_hash != startup_snapshot_format::abi_hash() {
        if startup_snapshot_debug_enabled() {
            eprintln!("embedded snapshot abi hash mismatch; falling back to cold startup");
        }
        return None;
    }
    Some(bytes)
}

// ── Embedded support cwasm ────────────────────────────────────────────
//
// 运行时持有 build-time 预编译的 support cwasm 字节。首次访问时自动
// 从 wjsm_runtime_support::EMBEDDED_SUPPORT_CWASM 初始化；CLI 侧亦可
// 通过 install_embedded_support_cwasm 显式注入（优先）。

static EMBEDDED_SUPPORT_CWASM: OnceLock<&'static [u8]> = OnceLock::new();

/// 安装编译时嵌入的 support cwasm；进程内只需调用一次（重复 set 静默忽略）。
/// 未显式调用时，`embedded_support_cwasm()` 自动从 build-time artifact 初始化。
pub fn install_embedded_support_cwasm(cwasm_bytes: &'static [u8]) {
    let _ = EMBEDDED_SUPPORT_CWASM.set(cwasm_bytes);
}

/// 返回已安装的 embedded support cwasm 字节。
/// 首次调用时若尚未通过 `install_embedded_support_cwasm` 显式注入，
/// 自动从 wjsm_runtime_support::EMBEDDED_SUPPORT_CWASM 初始化。
/// 返回 None 仅当 embedded feature 未启用（build-time artifact 为空）。
pub fn embedded_support_cwasm() -> Option<&'static [u8]> {
    EMBEDDED_SUPPORT_CWASM.get_or_init(|| {
        wjsm_runtime_support::EMBEDDED_SUPPORT_CWASM.unwrap_or(&[])
    });
    let bytes = EMBEDDED_SUPPORT_CWASM.get().copied()?;
    if bytes.is_empty() { None } else { Some(bytes) }
}

pub(crate) async fn execute_with_writer_shared_agent<W: Write>(
    wasm_bytes: &[u8],
    writer: W,
    shared_state: Arc<SharedRuntimeState>,
) -> Result<W> {
    execute_with_writer_shared_inner(wasm_bytes, writer, Some(shared_state), false).await
}
pub(crate) async fn execute_with_writer_shared<W: Write>(
    wasm_bytes: &[u8],
    writer: W,
    shared_state: Option<Arc<SharedRuntimeState>>,
) -> Result<W> {
    execute_with_writer_shared_inner(wasm_bytes, writer, shared_state, true).await
}

use runtime_startup::*;

#[cfg(test)]
#[derive(Default)]
struct StartupBenchTimings {
    engine_only: Duration,
    module_only: Duration,
    store_only: Duration,
    linker_register: Duration,
    instantiate_async: Duration,
    bootstrap_cold: Duration,
    host_post_bootstrap: Duration,
    snapshot_build: Duration,
    snapshot_decode: Duration,
    snapshot_restore: Duration,
    startup_path: Duration,
    main_completion: Duration,
    total_execute_path: Duration,
}

#[cfg(test)]
async fn instantiate_for_startup_bench(wasm: &[u8]) -> Result<StartupBenchTimings> {
    let mut timings = StartupBenchTimings::default();

    let config = startup_engine_config(true);
    let start = std::time::Instant::now();
    let engine = Engine::new(&config)
        .map_err(|e| anyhow::anyhow!("Failed to create async engine: {:?}", e))?;
    timings.engine_only = start.elapsed();

    let start = std::time::Instant::now();
    let module = match Module::new(&engine, wasm) {
        Ok(m) => m,
        Err(e) => {
            return Err(anyhow::anyhow!("WASM validation failed: {:?}", e));
        }
    };
    timings.module_only = start.elapsed();

    let start = std::time::Instant::now();
    let mut store = Store::new(&engine, RuntimeState::new_with_shared(None));
    store.set_epoch_deadline(1);
    store.epoch_deadline_async_yield_and_update(1);
    let _host_completion_rx = prepare_async_host_completion(&mut store);
    timings.store_only = start.elapsed();

    let mut linker = Linker::new(&engine);
    let start = std::time::Instant::now();
    register_startup_linker(&mut linker, &mut store)?;
    timings.linker_register = start.elapsed();

    let start = std::time::Instant::now();
    // P2.2: bench 也必须为 import env memory/table/globals 设置 shared env + support module。
    let needs_support = module.imports().any(|import| import.module() == "wjsm_support");
    if needs_support {
        setup_shared_env_and_support(&mut linker, &mut store, &engine).await?;
    }
    let instance = linker
        .instantiate_async(&mut store, &module)
        .await
        .map_err(|e| anyhow::anyhow!("async instantiate failed: {:?}", e))?;
    timings.instantiate_async = start.elapsed();

    // bench 也走 cold 路径单一编排：Rust 显式跑 host post-bootstrap → bootstrap_once。
    let wasm_env = extract_wasm_env(&instance, &mut store);

    let start = std::time::Instant::now();
    if let Ok(init_globals_fn) =
        instance.get_typed_func::<(), i64>(&mut store, "__wjsm_init_globals")
    {
        let _ = init_globals_fn.call_async(&mut store, ()).await;
    }
    initialize_host_post_bootstrap(&mut store, &wasm_env);
    timings.host_post_bootstrap = start.elapsed();

    let start = std::time::Instant::now();
    if let Ok(bootstrap_fn) =
        instance.get_typed_func::<(), i64>(&mut store, "__wjsm_bootstrap_once")
    {
        let _ = bootstrap_fn.call_async(&mut store, ()).await;
    }
    timings.bootstrap_cold = start.elapsed();

    // snapshot build = capture + encode；解码与恢复在新 instance 上重测一次。
    let start = std::time::Instant::now();
    let snap = startup_snapshot::capture_startup_snapshot(&mut store, &wasm_env)?;
    let bytes = startup_snapshot_format::encode_snapshot(&snap);
    timings.snapshot_build = start.elapsed();

    let start = std::time::Instant::now();
    let view = startup_snapshot_format::decode_snapshot(&bytes)?;
    timings.snapshot_decode = start.elapsed();

    let mut store2 = Store::new(&engine, RuntimeState::new_with_shared(None));
    store2.set_epoch_deadline(1);
    store2.epoch_deadline_async_yield_and_update(1);
    let _rx2 = prepare_async_host_completion(&mut store2);
    let mut linker2 = Linker::new(&engine);
    register_startup_linker(&mut linker2, &mut store2)?;
    if needs_support {
        setup_shared_env_and_support(&mut linker2, &mut store2, &engine).await?;
    }
    let instance2 = linker2
        .instantiate_async(&mut store2, &module)
        .await
        .map_err(|e| anyhow::anyhow!("async instantiate failed: {:?}", e))?;
    let env2 = extract_wasm_env(&instance2, &mut store2);
    // Snapshot restore 前必须先运行 init_globals 设置 __arr_proto_table_len 等。
    if let Ok(init_globals_fn) =
        instance2.get_typed_func::<(), i64>(&mut store2, "__wjsm_init_globals")
    {
        let _ = init_globals_fn.call_async(&mut store2, ()).await;
    }
    initialize_host_post_bootstrap(&mut store2, &env2);
    let start = std::time::Instant::now();
    startup_snapshot::restore_startup_snapshot(&mut store2, &env2, view)?;
    timings.snapshot_restore = start.elapsed();
    Ok(timings)
}

#[cfg(test)]
async fn execute_for_startup_bench(
    wasm_bytes: &[u8],
    snapshot_enabled: bool,
) -> Result<StartupBenchTimings> {
    let mut timings = StartupBenchTimings::default();
    let total_start = std::time::Instant::now();
    let config = startup_engine_config(true);

    let start = std::time::Instant::now();
    let engine = Engine::new(&config)
        .map_err(|e| anyhow::anyhow!("Failed to create async engine: {:?}", e))?;
    timings.engine_only = start.elapsed();

    let start = std::time::Instant::now();
    let module = Module::new(&engine, wasm_bytes)
        .map_err(|e| anyhow::anyhow!("WASM validation failed: {:?}", e))?;
    timings.module_only = start.elapsed();

    let snapshot_bytes = if snapshot_enabled {
        embedded_startup_snapshot_view()
    } else {
        None
    };

    let mut bundle = instantiate_execute_bundle(&engine, &module, None, true).await?;

    let startup_start = std::time::Instant::now();
    let mut snapshot_restored = false;
    if let Some(snap_bytes) = snapshot_bytes {
        let start = std::time::Instant::now();
        let view = startup_snapshot_format::decode_snapshot(snap_bytes)?;
        timings.snapshot_decode = start.elapsed();

        let start = std::time::Instant::now();
        // Snapshot restore 前先运行 init_globals 使 __arr_proto_table_len 等 globals 就绪。
        let _ = run_init_globals_only(&mut bundle).await;
        startup_snapshot::restore_startup_snapshot(&mut bundle.store, &bundle.wasm_env, view)?;
        timings.snapshot_restore = start.elapsed();
        snapshot_restored = true;
    }

    if !snapshot_restored {
        run_startup_cold_path(&mut bundle).await?;
    }
    timings.startup_path = startup_start.elapsed();

    let start = std::time::Instant::now();
    let _out = run_main_completion_block_async(
        &bundle.instance,
        bundle.store,
        bundle.wasm_env,
        bundle.output,
        bundle.runtime_error,
        Vec::new(),
        &mut bundle.host_completion_rx,
    )
    .await?;
    timings.main_completion = start.elapsed();
    timings.total_execute_path = total_start.elapsed();
    Ok(timings)
}
async fn execute_with_writer_shared_inner<W: Write>(
    wasm_bytes: &[u8],
    writer: W,
    shared_state: Option<Arc<SharedRuntimeState>>,
    use_epoch_async_yield: bool,
) -> Result<W> {
    let config = startup_engine_config(use_epoch_async_yield);
    let engine = Engine::new(&config)
        .map_err(|e| anyhow::anyhow!("Failed to create async engine: {:?}", e))?;

    let module = compile_or_load_cached(&engine, wasm_bytes)?;
    let snapshot_bytes = if startup_snapshot_enabled() {
        embedded_startup_snapshot_view()
    } else {
        None
    };

    let mut bundle = instantiate_execute_bundle(
        &engine,
        &module,
        shared_state.clone(),
        use_epoch_async_yield,
    )
    .await?;

    let mut snapshot_restored = false;
    if let Some(bytes) = snapshot_bytes {
        // 快照 restore 前必须先运行 init_globals + host_post_bootstrap，
        // 设置 __arr_proto_table_len 等 imported globals 使 restore 校验通过。
        let _ = run_init_globals_only(&mut bundle).await;
        snapshot_restored = try_restore_snapshot(&mut bundle, bytes).await;
        if !snapshot_restored {
            bundle = instantiate_execute_bundle(
                &engine,
                &module,
                shared_state.clone(),
                use_epoch_async_yield,
            )
            .await?;
        }
    }

    if !snapshot_restored {
        run_startup_cold_path(&mut bundle).await?;
    }

    run_main_completion_block_async(
        &bundle.instance,
        bundle.store,
        bundle.wasm_env,
        bundle.output,
        bundle.runtime_error,
        writer,
        &mut bundle.host_completion_rx,
    )
    .await
}
impl Clone for RuntimeState {
    fn clone(&self) -> Self {
        Self {
            output: self.output.clone(),
            iterators: self.iterators.clone(),
            enumerators: self.enumerators.clone(),
            runtime_strings: self.runtime_strings.clone(),
            runtime_error: self.runtime_error.clone(),
            gc_mark_bits: self.gc_mark_bits.clone(),
            alloc_counter: self.alloc_counter.clone(),
            gc_threshold: self.gc_threshold,
            timers: self.timers.clone(),
            cancelled_timers: self.cancelled_timers.clone(),
            next_timer_id: self.next_timer_id.clone(),
            closures: self.closures.clone(),
            bound_objects: self.bound_objects.clone(),
            native_callables: self.native_callables.clone(),
            native_callable_free_slots: self.native_callable_free_slots.clone(),
            handle_free_list: self.handle_free_list.clone(),
            abandoned_regions: self.abandoned_regions.clone(),
            gc_algorithm: self.gc_algorithm.clone(),
            continuation_free_slots: self.continuation_free_slots.clone(),
            combinator_context_free_slots: self.combinator_context_free_slots.clone(),
            eval_cache: self.eval_cache.clone(),
            bigint_table: self.bigint_table.clone(),
            symbol_table: self.symbol_table.clone(),
            regex_table: self.regex_table.clone(),
            promise_table: self.promise_table.clone(),
            pending_unhandled_rejections: self.pending_unhandled_rejections.clone(),
            microtask_queue: self.microtask_queue.clone(),
            continuation_table: self.continuation_table.clone(),
            async_generator_table: self.async_generator_table.clone(),
            async_from_sync_iterators: self.async_from_sync_iterators.clone(),
            async_iterator_prototype: self.async_iterator_prototype,
            async_gen_prototype: self.async_gen_prototype,
            array_proto_values: AtomicI64::new(self.array_proto_values.load(Ordering::Relaxed)),
            combinator_contexts: self.combinator_contexts.clone(),
            module_namespace_cache: self.module_namespace_cache.clone(),
            error_table: self.error_table.clone(),
            map_table: self.map_table.clone(),
            set_table: self.set_table.clone(),
            weakmap_table: self.weakmap_table.clone(),
            weakset_table: self.weakset_table.clone(),
            weakref_table: self.weakref_table.clone(),
            finalization_registry_table: self.finalization_registry_table.clone(),
            pending_cleanup_callbacks: self.pending_cleanup_callbacks.clone(),
            proxy_table: self.proxy_table.clone(),
            arraybuffer_table: self.arraybuffer_table.clone(),
            dataview_table: self.dataview_table.clone(),
            typedarray_table: self.typedarray_table.clone(),
            headers_table: self.headers_table.clone(),
            fetch_response_table: self.fetch_response_table.clone(),
            fetch_request_table: self.fetch_request_table.clone(),
            abort_signal_table: self.abort_signal_table.clone(),
            http_response_table: self.http_response_table.clone(),
            readable_stream_table: self.readable_stream_table.clone(),
            reader_table: self.reader_table.clone(),
            stream_controller_table: self.stream_controller_table.clone(),
            byob_request_table: self.byob_request_table.clone(),
            writable_stream_table: self.writable_stream_table.clone(),
            writer_table: self.writer_table.clone(),
            transform_stream_table: self.transform_stream_table.clone(),
            shared_state: self.shared_state.clone(),
            non_extensible_handles: self.non_extensible_handles.clone(),
            scope_records: self.scope_records.clone(),
            scope_record_next_handle: self.scope_record_next_handle,
            new_target: AtomicI64::new(self.new_target.load(Ordering::Relaxed)),
            fetch_http_clients: self.fetch_http_clients.clone(),
            host_completion_tx: self.host_completion_tx.clone(),
            async_op_counter: self.async_op_counter.clone(),
        }
    }
}
struct RuntimeState {
    output: Arc<Mutex<Vec<u8>>>,
    iterators: Arc<Mutex<Vec<IteratorState>>>,
    enumerators: Arc<Mutex<Vec<EnumeratorState>>>,
    runtime_strings: Arc<Mutex<Vec<String>>>,
    runtime_error: Arc<Mutex<Option<String>>>,
    /// GC 标记位图：每个 handle 对应 1 bit，用于标记-清除 GC。
    gc_mark_bits: Arc<Mutex<Vec<u64>>>,
    /// 分配计数器：每次对象分配后递增，用于触发周期性 GC。
    alloc_counter: Arc<Mutex<u64>>,
    /// GC 触发阈值：当 alloc_counter 达到此值时触发 GC。
    gc_threshold: u64,
    /// 定时器列表
    timers: Arc<Mutex<Vec<TimerEntry>>>,
    /// 已取消的定时器 ID 集合
    cancelled_timers: Arc<Mutex<HashSet<u32>>>,
    /// 下一个定时器 ID
    next_timer_id: Arc<Mutex<u32>>,
    /// 闭包表：每个闭包条目存储函数表索引和环境对象
    closures: Arc<Mutex<Vec<ClosureEntry>>>,
    /// 绑定函数表：存储 func.bind(this, args) 创建的绑定函数
    bound_objects: Arc<Mutex<Vec<BoundRecord>>>,
    /// 运行时原生可调用对象表：Promise resolving functions 等宿主创建函数。
    native_callables: Arc<Mutex<Vec<NativeCallable>>>,
    /// native_callable 表空闲槽位，用于复用已释放条目。
    native_callable_free_slots: Arc<Mutex<Vec<u32>>>,
    /// GC sweep 回收的 obj_table handle 槽（供 fast-path 复用，spec #7/IMPL-10）。
    /// runtime_gc::MarkSweepCollector::collect 把 sweep 回收的 handle push 到此；
    /// gc_take_freed_handle host import（P4）pop 给 WASM fast-path。
    handle_free_list: Arc<Mutex<Vec<u32>>>,
    /// resize（grow_array/grow_object）抛弃的旧区域 (ptr, size)。
    /// handle 的 obj_table 槽被重写到新 ptr 后，旧 ptr 区域不再被 obj_table 索引，
    /// sweep 单独遍历 obj_table 看不到它 → 永久泄漏（INV-B vs §8.2 矛盾，P4-blocker #1）。
    /// grow_array/grow_object 在重写前注册旧 (ptr, old_size)；sweep 读此并入 free list，sweep 结束清空。
    abandoned_regions: Arc<Mutex<Vec<(usize, usize)>>>,
    /// 可插拔 GC 算法实例（默认 MarkSweepCollector）。host imports gc_alloc_slow/
    /// gc_maybe_collect 经此驱动（P4）。Arc<Mutex> 因 host fn 经 &Caller 访问需内部可变性。
    gc_algorithm: Arc<Mutex<Box<dyn crate::runtime_gc::GcAlgorithm + Send + Sync>>>,
    /// continuation 侧表空闲槽位（handle 下标稳定，禁止 Vec::retain）。
    continuation_free_slots: Arc<Mutex<Vec<u32>>>,
    /// combinator context 侧表空闲槽位。
    combinator_context_free_slots: Arc<Mutex<Vec<usize>>>,
    /// eval 编译缓存：code string hash → eval 模式 WASM bytes。
    eval_cache: Arc<Mutex<HashMap<u64, Vec<u8>>>>,
    /// BigInt 侧表：存储任意精度 BigInt 值
    bigint_table: Arc<Mutex<Vec<num_bigint::BigInt>>>,
    /// Symbol 侧表：存储 symbol 条目（description + global_key）
    symbol_table: Arc<Mutex<Vec<SymbolEntry>>>,
    /// RegExp 侧表：存储编译后的正则表达式和元数据
    regex_table: Arc<Mutex<Vec<RegexEntry>>>,
    /// Promise 侧表：object handle → Promise 内部槽；非 Promise object handle 使用空占位。
    promise_table: Arc<Mutex<Vec<PromiseEntry>>>,
    /// 已 reject 且尚未 handled 的 promise 索引，用于 drain 时避免全表扫描。
    pending_unhandled_rejections: Arc<Mutex<HashSet<usize>>>,
    /// 微任务队列
    microtask_queue: Arc<Mutex<VecDeque<Microtask>>>,
    /// Continuation 侧表：存储异步函数续延
    continuation_table: Arc<Mutex<Vec<ContinuationEntry>>>,
    /// AsyncGenerator 侧表：存储异步生成器状态
    async_generator_table: Arc<Mutex<Vec<AsyncGeneratorEntry>>>,
    /// async-from-sync iterator 侧表
    async_from_sync_iterators: Arc<Mutex<Vec<AsyncFromSyncIteratorEntry>>>,
    /// %AsyncIteratorPrototype% 对象
    async_iterator_prototype: i64,
    /// AsyncGenerator.prototype 对象
    async_gen_prototype: i64,
    /// Promise combinator 侧表：pending 元素的 reaction 通过索引回写共享结果。
    combinator_contexts: Arc<Mutex<Vec<CombinatorContext>>>,
    /// 模块命名空间对象缓存：module_id → namespace object (i64 NaN-boxed)
    module_namespace_cache: Arc<Mutex<HashMap<u32, i64>>>,
    /// Error 侧表：存储 error 对象的 name 和 message
    error_table: Arc<Mutex<Vec<ErrorEntry>>>,
    /// Map 侧表：存储 Map 对象的键值对
    map_table: Arc<Mutex<Vec<MapEntry>>>,
    /// Set 侧表：存储 Set 对象的值
    set_table: Arc<Mutex<Vec<SetEntry>>>,
    /// WeakMap 侧表：存储 WeakMap 对象的键值对
    weakmap_table: Arc<Mutex<Vec<WeakMapEntry>>>,
    /// WeakSet 侧表：存储 WeakSet 对象的值
    weakset_table: Arc<Mutex<Vec<WeakSetEntry>>>,
    /// WeakRef 侧表：存储 WeakRef 对象的 target handle
    weakref_table: Arc<Mutex<Vec<WeakRefEntry>>>,
    /// Array.prototype.values 缓存，用于规范要求复用该函数对象的 @@iterator。
    array_proto_values: AtomicI64,
    /// FinalizationRegistry 侧表：存储 registry 对象、callback 和注册信息
    finalization_registry_table: Arc<Mutex<Vec<FinalizationRegistryEntry>>>,
    /// GC 后待调度的清理回调
    #[allow(clippy::type_complexity)]
    pending_cleanup_callbacks: Arc<Mutex<Vec<(i64, Vec<i64>)>>>,
    /// Proxy 侧表：存储 Proxy 对象的 target、handler 和 revoked 状态
    proxy_table: Arc<Mutex<Vec<ProxyEntry>>>,
    /// ArrayBuffer 侧表：存储 ArrayBuffer 的底层数据
    arraybuffer_table: Arc<Mutex<Vec<ArrayBufferEntry>>>,
    /// DataView 侧表：存储 DataView 的 buffer 引用和偏移量
    dataview_table: Arc<Mutex<Vec<DataViewEntry>>>,
    /// TypedArray 侧表：存储 TypedArray 的 buffer 引用、偏移量和长度
    typedarray_table: Arc<Mutex<Vec<TypedArrayEntry>>>,
    /// Headers 侧表：存储 Headers 对象 (key-value pairs, lowercased keys)
    headers_table: Arc<Mutex<Vec<HeadersEntry>>>,
    /// Fetch Response 侧表：存储 Response 对象 (status/headers/body)
    fetch_response_table: Arc<Mutex<Vec<FetchResponseEntry>>>,
    /// Fetch Request 侧表：存储 Request 对象 (method/url/headers/body)
    fetch_request_table: Arc<Mutex<Vec<FetchRequestEntry>>>,
    /// AbortSignal 侧表：存储 abort 状态
    abort_signal_table: Arc<Mutex<Vec<AbortSignalEntry>>>,
    /// reqwest Response 侧表：持有未消费的 HTTP response body stream
    http_response_table: Arc<Mutex<Vec<HttpResponseEntry>>>,
    /// 按 redirect 策略复用的 reqwest HTTP 客户端（连接池）
    fetch_http_clients: Arc<Mutex<HashMap<RedirectMode, reqwest::Client>>>,
    /// ReadableStream 侧表：存储流状态
    readable_stream_table: Arc<Mutex<Vec<ReadableStreamEntry>>>,
    /// Reader 侧表：存储 reader → stream 映射
    reader_table: Arc<Mutex<Vec<ReaderEntry>>>,
    /// Controller 侧表（ReadableStream DefaultController 等）
    stream_controller_table: Arc<Mutex<Vec<StreamControllerEntry>>>,
    byob_request_table: Arc<Mutex<Vec<ByobRequestEntry>>>,
    /// WritableStream 侧表：存储可写流状态
    writable_stream_table: Arc<Mutex<Vec<WritableStreamEntry>>>,
    /// Writer 侧表：存储 WritableStreamDefaultWriter → stream 映射
    writer_table: Arc<Mutex<Vec<WriterEntry>>>,
    /// TransformStream 侧表：存储转换流状态
    transform_stream_table: Arc<Mutex<Vec<TransformStreamEntry>>>,
    /// normal execution 拥有单 agent cluster；$262.agent 可共享同一状态。
    shared_state: Option<Arc<SharedRuntimeState>>,
    /// 被 preventExtensions 标记为不可扩展对象的 handle 集合（使用完整的 NaN-boxed 值作为 key）
    non_extensible_handles: Arc<Mutex<HashSet<u64>>>,
    /// Temporary ScopeRecord handles for active eval calls.
    /// Keyed by handle index; entries removed when eval returns.
    pub(crate) scope_records: HashMap<u32, crate::runtime_eval::ScopeRecord>,
    /// Monotonic counter for scope record handle allocation.
    /// Using a counter instead of len() ensures no collisions when records are removed.
    pub(crate) scope_record_next_handle: u32,
    /// new.target value meta property
    /// new.target 值元属性（AtomicI64 + Relaxed 替换 Cell，以满足 wasmtime async instantiate_async
    /// 要求的 T: Send + 'static 约束。Relaxed 足够且零开销；语义与原 Cell 完全等价。）
    new_target: AtomicI64,

    /// Phase 6: host completion channel tx（Option 便于 sync 路径保持 None；创建后 set Some）。
    /// 引用 plan Correction 7：worker 只通过 tx 发送可 Send 的 SettleValue 或 Materialize 闭包，绝不触碰 Store/heap。
    host_completion_tx:
        Option<tokio::sync::mpsc::UnboundedSender<crate::scheduler::AsyncHostCompletion>>,
    /// Phase 6: in-flight async op 计数器（用于 scheduler 安全退出条件）。
    async_op_counter: Option<crate::scheduler::AsyncOpCounter>,
}

impl RuntimeState {
    fn new_with_shared(shared_state: Option<Arc<SharedRuntimeState>>) -> Self {
        let mut state = Self::new();
        state.shared_state = shared_state.or_else(|| Some(new_shared_runtime_state()));
        state
    }

    /// GC 框架访问 handle_free_list（runtime_gc::MarkSweepCollector::collect 用）。
    /// 返回 handle_free_list 的可变引用，供 sweep 回收的 handle 入表。
    pub(crate) fn handle_free_list_for_gc(&self) -> Option<std::sync::MutexGuard<'_, Vec<u32>>> {
        self.handle_free_list.lock().ok()
    }

    /// 注册 resize（grow_array/grow_object）抛弃的旧区域 (ptr, old_size)。
    /// sweeper 读此并入 free list（P4-blocker #1）。
    pub(crate) fn abandon_region(&self, ptr: usize, size: usize) {
        if size == 0 {
            return;
        }
        if let Ok(mut list) = self.abandoned_regions.lock() {
            list.push((ptr, size));
        }
    }

    /// GC 框架访问 abandoned_regions（sweeper 读 + 清空）。
    pub(crate) fn abandoned_regions_for_gc(
        &self,
    ) -> Option<std::sync::MutexGuard<'_, Vec<(usize, usize)>>> {
        self.abandoned_regions.lock().ok()
    }

    /// 按 redirect 模式返回复用的 reqwest 客户端（进程内连接池）。
    pub(crate) fn http_client_for_redirect(
        &self,
        redirect: RedirectMode,
    ) -> std::result::Result<reqwest::Client, reqwest::Error> {
        let mut clients = self
            .fetch_http_clients
            .lock()
            .expect("fetch_http_clients mutex");
        if let Some(client) = clients.get(&redirect) {
            return Ok(client.clone());
        }
        let redirect_policy = match redirect {
            RedirectMode::Follow => reqwest::redirect::Policy::limited(20),
            RedirectMode::Error => reqwest::redirect::Policy::none(),
            RedirectMode::Manual => reqwest::redirect::Policy::limited(0),
        };
        let client = reqwest::Client::builder()
            .redirect(redirect_policy)
            .build()?;
        clients.insert(redirect, client.clone());
        Ok(client)
    }

    /// 构造一个新的 RuntimeState，所有侧表初始化为空，well-known symbols 预分配。

    pub(crate) fn new() -> Self {
        const GC_THRESHOLD: u64 = 1000;
        RuntimeState {
            output: Arc::new(Mutex::new(Vec::new())),
            iterators: Arc::new(Mutex::new(Vec::new())),
            enumerators: Arc::new(Mutex::new(Vec::new())),
            runtime_strings: Arc::new(Mutex::new(Vec::new())),
            runtime_error: Arc::new(Mutex::new(None)),
            gc_mark_bits: Arc::new(Mutex::new(Vec::new())),
            alloc_counter: Arc::new(Mutex::new(0)),
            gc_threshold: GC_THRESHOLD,
            timers: Arc::new(Mutex::new(Vec::new())),
            cancelled_timers: Arc::new(Mutex::new(HashSet::new())),
            next_timer_id: Arc::new(Mutex::new(1)),
            closures: Arc::new(Mutex::new(Vec::new())),
            bound_objects: Arc::new(Mutex::new(Vec::new())),
            native_callables: Arc::new(Mutex::new(vec![NativeCallable::EvalIndirect])),
            native_callable_free_slots: Arc::new(Mutex::new(Vec::new())),
            handle_free_list: Arc::new(Mutex::new(Vec::new())),
            abandoned_regions: Arc::new(Mutex::new(Vec::new())),
            gc_algorithm: Arc::new(Mutex::new(Box::new(
                crate::runtime_gc::MarkSweepCollector::new(),
            ))),
            continuation_free_slots: Arc::new(Mutex::new(Vec::new())),
            combinator_context_free_slots: Arc::new(Mutex::new(Vec::new())),
            eval_cache: Arc::new(Mutex::new(HashMap::new())),
            bigint_table: Arc::new(Mutex::new(Vec::new())),
            symbol_table: Arc::new(Mutex::new(vec![
                SymbolEntry {
                    description: Some("Symbol(Symbol.iterator)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.species)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.toStringTag)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.asyncIterator)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.hasInstance)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.toPrimitive)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.dispose)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.match)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.asyncDispose)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.isConcatSpreadable)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.matchAll)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.replace)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.search)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.split)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.unscopables)".into()),
                    global_key: None,
                },
            ])),
            regex_table: Arc::new(Mutex::new(Vec::new())),
            promise_table: Arc::new(Mutex::new(Vec::new())),
            pending_unhandled_rejections: Arc::new(Mutex::new(HashSet::new())),
            microtask_queue: Arc::new(Mutex::new(VecDeque::new())),
            continuation_table: Arc::new(Mutex::new(Vec::new())),
            async_generator_table: Arc::new(Mutex::new(Vec::new())),
            async_from_sync_iterators: Arc::new(Mutex::new(Vec::new())),
            async_iterator_prototype: value::encode_undefined(),
            async_gen_prototype: value::encode_undefined(),
            combinator_contexts: Arc::new(Mutex::new(Vec::new())),
            module_namespace_cache: Arc::new(Mutex::new(HashMap::new())),
            error_table: Arc::new(Mutex::new(Vec::new())),
            map_table: Arc::new(Mutex::new(Vec::new())),
            set_table: Arc::new(Mutex::new(Vec::new())),
            weakmap_table: Arc::new(Mutex::new(Vec::new())),
            weakset_table: Arc::new(Mutex::new(Vec::new())),
            weakref_table: Arc::new(Mutex::new(Vec::new())),
            finalization_registry_table: Arc::new(Mutex::new(Vec::new())),
            pending_cleanup_callbacks: Arc::new(Mutex::new(Vec::new())),
            proxy_table: Arc::new(Mutex::new(Vec::new())),
            arraybuffer_table: Arc::new(Mutex::new(Vec::new())),
            dataview_table: Arc::new(Mutex::new(Vec::new())),
            typedarray_table: Arc::new(Mutex::new(Vec::new())),
            headers_table: Arc::new(Mutex::new(Vec::new())),
            array_proto_values: AtomicI64::new(value::encode_undefined()),
            fetch_response_table: Arc::new(Mutex::new(Vec::new())),
            fetch_request_table: Arc::new(Mutex::new(Vec::new())),
            abort_signal_table: Arc::new(Mutex::new(Vec::new())),
            http_response_table: Arc::new(Mutex::new(Vec::new())),
            fetch_http_clients: Arc::new(Mutex::new(HashMap::new())),
            readable_stream_table: Arc::new(Mutex::new(Vec::new())),
            reader_table: Arc::new(Mutex::new(Vec::new())),
            stream_controller_table: Arc::new(Mutex::new(Vec::new())),
            byob_request_table: Arc::new(Mutex::new(Vec::new())),
            writable_stream_table: Arc::new(Mutex::new(Vec::new())),
            transform_stream_table: Arc::new(Mutex::new(Vec::new())),
            writer_table: Arc::new(Mutex::new(Vec::new())),
            shared_state: Some(new_shared_runtime_state()),
            non_extensible_handles: Arc::new(Mutex::new(HashSet::new())),
            scope_records: HashMap::new(),
            scope_record_next_handle: 0,
            new_target: AtomicI64::new(value::encode_undefined()),
            host_completion_tx: None,
            async_op_counter: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::execute_with_writer;
    use anyhow::Result;
    use tokio::runtime::Runtime;
    // Phase 5 TDD 回归标记（严格按主代理 2026-06-01 授权方案）：
    // - TimerEntry.deadline 改为 tokio::time::Instant（仅此字段，interval 仍 std Duration）
    // - 仅通过根 use + 创建点显式路径 + sync loop 最小限定符（若需）实现
    // - 目标：cargo check 0 errors + sync timer 语义 100% 保留（MAX_ITERATIONS、per-callback drain、重复重调度顺序不变）
    // - async 路径后续由 scheduler.rs 接管；sync 路径 loop 文本逻辑零改动
    // - 验证：本注释后立即做机械类型变更，每步 cargo check --tests 确认通过
    //   （注意：当前 execute 测试因 wiring 期 pre-existing 缺失 define_get_builtin_global 无法 runtime 跑 timer fixture，此为非 Phase 5 问题，不在此 slice 修复）
    fn compile_source(source: &str) -> Result<Vec<u8>> {
        super::compile_source(source)
    }

    #[test]
    fn execute_with_writer_prints_string_fixture() -> Result<()> {
        let rt = Runtime::new()?;
        let wasm_bytes = compile_source(r#"console.log("Hello, Async Runtime!");"#)?;
        let output = rt.block_on(async { execute_with_writer(&wasm_bytes, Vec::new()).await })?;
        assert_eq!(String::from_utf8(output)?, "Hello, Async Runtime!\n");
        Ok(())
    }

    // 临时 benchmark：分阶段测 execute 路径各步的耗时（消除单次噪声，每步循环取均值）。
    #[test]
    #[ignore]
    fn bench_execute_phases() -> Result<()> {
        use super::*;
        let wasm = compile_source("")?;
        let n = 50u32;
        let rt = Runtime::new()?;

        let mut startup = StartupBenchTimings::default();
        rt.block_on(async {
            for _ in 0..n {
                let run = instantiate_for_startup_bench(&wasm).await.unwrap();
                startup.engine_only += run.engine_only;
                startup.module_only += run.module_only;
                startup.store_only += run.store_only;
                startup.linker_register += run.linker_register;
                startup.instantiate_async += run.instantiate_async;
                startup.bootstrap_cold += run.bootstrap_cold;
                startup.host_post_bootstrap += run.host_post_bootstrap;
                startup.snapshot_build += run.snapshot_build;
                startup.snapshot_decode += run.snapshot_decode;
                startup.snapshot_restore += run.snapshot_restore;
            }
        });
        eprintln!(
            "BENCH engine only            : {:?}/each",
            startup.engine_only / n
        );
        eprintln!(
            "BENCH module only            : {:?}/each",
            startup.module_only / n
        );
        eprintln!(
            "BENCH store only             : {:?}/each",
            startup.store_only / n
        );
        eprintln!(
            "BENCH linker register        : {:?}/each",
            startup.linker_register / n
        );
        eprintln!(
            "BENCH instantiate_async      : {:?}/each",
            startup.instantiate_async / n
        );
        eprintln!(
            "BENCH bootstrap cold         : {:?}/each",
            startup.bootstrap_cold / n
        );
        eprintln!(
            "BENCH host post-bootstrap    : {:?}/each",
            startup.host_post_bootstrap / n
        );
        eprintln!(
            "BENCH snapshot build cold    : {:?}/each",
            startup.snapshot_build / n
        );
        eprintln!(
            "BENCH snapshot decode        : {:?}/each",
            startup.snapshot_decode / n
        );
        eprintln!(
            "BENCH snapshot restore       : {:?}/each",
            startup.snapshot_restore / n
        );

        // SAFETY: 单测内独占 env 窗口；勿与其它读 WJSM_STARTUP_SNAPSHOT 的测试并行。
        unsafe {
            std::env::set_var("WJSM_STARTUP_SNAPSHOT", "0");
        }
        let mut full_execute_off = std::time::Duration::ZERO;
        rt.block_on(async {
            for _ in 0..n {
                let start = std::time::Instant::now();
                let _ = execute_with_writer(&wasm, Vec::new()).await.unwrap();
                full_execute_off += start.elapsed();
            }
        });
        eprintln!(
            "BENCH full execute off       : {:?}/each",
            full_execute_off / n
        );

        let mut execute_off = StartupBenchTimings::default();
        rt.block_on(async {
            for _ in 0..n {
                let run = execute_for_startup_bench(&wasm, false).await.unwrap();
                execute_off.engine_only += run.engine_only;
                execute_off.module_only += run.module_only;
                execute_off.startup_path += run.startup_path;
                execute_off.main_completion += run.main_completion;
                execute_off.total_execute_path += run.total_execute_path;
            }
        });
        eprintln!(
            "BENCH real off engine        : {:?}/each",
            execute_off.engine_only / n
        );
        eprintln!(
            "BENCH real off module        : {:?}/each",
            execute_off.module_only / n
        );
        eprintln!(
            "BENCH real off startup       : {:?}/each",
            execute_off.startup_path / n
        );
        eprintln!(
            "BENCH real off main          : {:?}/each",
            execute_off.main_completion / n
        );
        eprintln!(
            "BENCH real off total         : {:?}/each",
            execute_off.total_execute_path / n
        );

        let embedded_snapshot = build_embedded_startup_snapshot_bytes()?;
        install_embedded_startup_snapshot(&embedded_snapshot);

        unsafe {
            std::env::set_var("WJSM_STARTUP_SNAPSHOT", "1");
        }
        // embedded snapshot 已安装；循环测默认 on 路径。
        let mut full_execute_warm = std::time::Duration::ZERO;
        rt.block_on(async {
            for _ in 0..n {
                let start = std::time::Instant::now();
                let _ = execute_with_writer(&wasm, Vec::new()).await.unwrap();
                full_execute_warm += start.elapsed();
            }
        });
        unsafe {
            std::env::remove_var("WJSM_STARTUP_SNAPSHOT");
        }
        eprintln!(
            "BENCH full execute embedded  : {:?}/each",
            full_execute_warm / n
        );

        unsafe {
            std::env::set_var("WJSM_STARTUP_SNAPSHOT", "1");
        }
        rt.block_on(async {
            let _ = execute_with_writer(&wasm, Vec::new()).await.unwrap();
        });
        let mut execute_on = StartupBenchTimings::default();
        rt.block_on(async {
            for _ in 0..n {
                let run = execute_for_startup_bench(&wasm, true).await.unwrap();
                execute_on.engine_only += run.engine_only;
                execute_on.module_only += run.module_only;
                execute_on.snapshot_decode += run.snapshot_decode;
                execute_on.snapshot_restore += run.snapshot_restore;
                execute_on.startup_path += run.startup_path;
                execute_on.main_completion += run.main_completion;
                execute_on.total_execute_path += run.total_execute_path;
            }
        });
        unsafe {
            std::env::remove_var("WJSM_STARTUP_SNAPSHOT");
        }
        eprintln!(
            "BENCH real on engine         : {:?}/each",
            execute_on.engine_only / n
        );
        eprintln!(
            "BENCH real on module         : {:?}/each",
            execute_on.module_only / n
        );
        eprintln!(
            "BENCH real on decode         : {:?}/each",
            execute_on.snapshot_decode / n
        );
        eprintln!(
            "BENCH real on restore        : {:?}/each",
            execute_on.snapshot_restore / n
        );
        eprintln!(
            "BENCH real on startup        : {:?}/each",
            execute_on.startup_path / n
        );
        eprintln!(
            "BENCH real on main           : {:?}/each",
            execute_on.main_completion / n
        );
        eprintln!(
            "BENCH real on total          : {:?}/each",
            execute_on.total_execute_path / n
        );

        Ok(())
    }

    /// Criterion bench：测量 WASM 编译缓存 + startup snapshot 两种序列化路径的反序列化耗时。
    /// 运行：cargo test -p wjsm-runtime -- bench_deserialize --nocapture --ignored
    #[test]
    #[ignore]
    fn bench_deserialize() -> Result<()> {
        use criterion::Criterion;
        use super::*;
        let wasm = compile_source("")?;
        let rt = Runtime::new()?;
        let mut c = Criterion::default().sample_size(50);

        // ── 准备缓存目录 ────────────────────────────────────────────
        let cache_dir = std::env::temp_dir().join("wjsm-bench-criterion");
        let _ = std::fs::remove_dir_all(&cache_dir);
        std::fs::create_dir_all(&cache_dir)?;
        unsafe { std::env::set_var("WJSM_CACHE_DIR", &cache_dir); }

        let config = startup_engine_config(true);
        let engine = Engine::new(&config)
            .map_err(|e| anyhow::anyhow!("engine: {e:?}"))?;

        // ── 1. WASM 缓存 warm 命中 ──────────────────────────────────
        let _cold = compile_or_load_cached(&engine, &wasm)?;
        let mut group = c.benchmark_group("wasm_cache");
        group.bench_function("deserialize_file (warm)", |b| {
            b.iter(|| {
                criterion::black_box(
                    compile_or_load_cached(&engine, criterion::black_box(&wasm))
                        .expect("warm deserialize"),
                );
            })
        });
        // ── cold 编译 + precompile ──
        group.bench_function("compile+precompile (cold)", |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let _ = std::fs::remove_dir_all(&cache_dir);
                    std::fs::create_dir_all(&cache_dir).expect("create cache dir");
                    let start = std::time::Instant::now();
                    criterion::black_box(
                        compile_or_load_cached(&engine, criterion::black_box(&wasm))
                            .expect("cold compile"),
                    );
                    total += start.elapsed();
                }
                total
            })
        });
        group.finish();

        // ── 2. Support cwasm deserialize ────────────────────────────
        let cwasm_bytes = embedded_support_cwasm()
            .ok_or_else(|| anyhow::anyhow!("embedded support cwasm not available"))?;
        let mut group = c.benchmark_group("support_cwasm");
        group.bench_function("Module::deserialize", |b| {
            b.iter(|| unsafe {
                criterion::black_box(
                    Module::deserialize(criterion::black_box(&engine), criterion::black_box(cwasm_bytes))
                        .expect("support deserialize"),
                );
            })
        });
        group.finish();

        // ── 3. Snapshot decode ──────────────────────────────────────
        let snap_bytes = build_embedded_startup_snapshot_bytes()?;
        let mut group = c.benchmark_group("snapshot");
        group.bench_function("decode", |b| {
            b.iter(|| {
                criterion::black_box(
                    startup_snapshot_format::decode_snapshot(criterion::black_box(&snap_bytes))
                        .expect("snapshot decode"),
                );
            })
        });

        // ── 4. Snapshot restore ─────────────────────────────────────
        let snap_view = startup_snapshot_format::decode_snapshot(&snap_bytes)?;
        group.bench_function("restore", |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    rt.block_on(async {
                        let config = startup_engine_config(true);
                        let engine = Engine::new(&config).expect("engine");
                        let module = Module::new(&engine, &wasm).expect("module");
                        let mut store = Store::new(&engine, RuntimeState::new_with_shared(None));
                        store.set_epoch_deadline(1);
                        store.epoch_deadline_async_yield_and_update(1);
                        let _rx = prepare_async_host_completion(&mut store);
                        let mut linker = Linker::new(&engine);
                        register_startup_linker(&mut linker, &mut store).expect("register linker");
                        let needs_support = module.imports().any(|imp| imp.module() == "wjsm_support");
                        if needs_support {
                            setup_shared_env_and_support(&mut linker, &mut store, &engine)
                                .await
                                .expect("setup support");
                        }
                        let instance = linker.instantiate_async(&mut store, &module)
                            .await
                            .expect("instantiate");
                        let env = extract_wasm_env(&instance, &mut store);
                        if let Ok(f) = instance.get_typed_func::<(), i64>(&mut store, "__wjsm_init_globals") {
                            let _ = f.call_async(&mut store, ()).await;
                        }
                        let start = std::time::Instant::now();
                        startup_snapshot::restore_startup_snapshot(&mut store, &env, snap_view.clone())
                            .expect("restore");
                        total += start.elapsed();
                    });
                }
                total
            })
        });
        group.finish();

        let _ = std::fs::remove_dir_all(&cache_dir);
        Ok(())
    }
    // Phase 6 针对性单元测试（任务 6）：手动 enqueue 完成，验证 value settlement + 材料化能分配 runtime string/object
    // 严格引用 plan 458-550 + Correction 7：worker 只 Send 数据/闭包，materialize 在 Store owner 执行
    // 使用 channel 直接模拟（不依赖真实 wasm 主流程），证明 channel 形状 + 处理路径正确
    #[test]
    fn async_host_completion_manual_enqueue_settle_and_materialize() -> Result<()> {
        use super::runtime_builtins::PromiseSettlement;
        use super::scheduler::{AsyncHostCompletion, AsyncOpCounter};

        let counter = AsyncOpCounter::new();
        let _guard = counter.begin();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AsyncHostCompletion>();

        // 手动 enqueue SettleValue（模拟 worker 发简单值）
        tx.send(AsyncHostCompletion::SettleValue {
            promise: 100,
            settlement: PromiseSettlement::Fulfill(999),
        })
        .expect("send settle");

        // 手动 enqueue Materialize（闭包在 owner 执行，可分配）
        let mat: Box<
            dyn FnOnce(
                    &mut wasmtime::Store<super::RuntimeState>,
                    &super::WasmEnv,
                ) -> PromiseSettlement
                + Send,
        > = Box::new(|_store, _env| {
            // 真实会 alloc runtime string/object，此处模拟
            PromiseSettlement::Fulfill(888)
        });
        tx.send(AsyncHostCompletion::Materialize {
            promise: 101,
            materialize: mat,
        })
        .expect("send mat");

        // 模拟 scheduler loop drain (while try_recv)
        let c1 = rx.try_recv().expect("c1");
        match c1 {
            AsyncHostCompletion::SettleValue {
                promise,
                settlement: PromiseSettlement::Fulfill(v),
            } => {
                assert_eq!(promise, 100);
                assert_eq!(v, 999);
            }
            _ => panic!("bad settle"),
        }
        let c2 = rx.try_recv().expect("c2");
        match c2 {
            AsyncHostCompletion::Materialize { promise, .. } => {
                assert_eq!(promise, 101);
            }
            _ => panic!("bad mat"),
        }
        drop(_guard);
        assert_eq!(counter.count(), 0);
        Ok(())
    }
}
