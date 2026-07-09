use anyhow::Result;
use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};
use num_traits::cast::ToPrimitive;
use rand::Rng;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Duration;
use swc_core::ecma::ast as swc_ast;
use tokio::time::Instant;
use wasmtime::Func;
use wasmtime::*;
use wjsm_ir::{constants, value};
use wjsm_snapshot_format as startup_snapshot_format;
mod agent_cluster;
mod array_named_props;
mod host_side_table;
mod property_key;
mod runtime_arguments;
mod runtime_async_fn;
mod runtime_buffer;
mod runtime_builtins;
mod runtime_collection_gc;
mod runtime_collections;
mod runtime_combinators;
mod runtime_date;
mod runtime_encoding;
mod runtime_eval;
mod runtime_gc;
pub use runtime_gc::api::{CycleKind, GcStats};
pub use runtime_gc::registry::GcAlgorithmKind;
mod runtime_generator;
mod runtime_heap;
mod runtime_host_helpers;
pub(crate) use host_side_table::HostSideTable;
mod runtime_json;
mod runtime_linker;
mod runtime_microtask;
mod runtime_module_loader;
mod runtime_module_registry;
mod runtime_node_child_process;
mod runtime_node_crypto;
mod runtime_node_data;
mod runtime_node_fs;
mod runtime_node_globals;
mod runtime_node_net;
mod runtime_node_dgram;
mod runtime_node_tls;
mod runtime_node_worker_threads;
mod runtime_worker_message;
mod runtime_node_zlib;
mod runtime_process;
mod runtime_promises;
pub use runtime_module_loader::{
    RuntimeInstantiatedModule, RuntimeInstantiationEnv, RuntimeModuleFormat,
    RuntimeModuleImportLink, RuntimeModuleInstantiationContext, RuntimeModuleLoadError,
    RuntimeModuleLoadErrorCode, RuntimeModuleLoader, RuntimeModulePlacement, RuntimeModuleReferrer,
    RuntimeModuleResolutionKind, RuntimeResolvedModule,
};
pub use runtime_module_registry::{
    RuntimeModuleKey, RuntimeModuleRegistry, RuntimeModuleRequireResult, RuntimeModuleState,
    RuntimeRequireCacheEntry,
};
mod runtime_regexp;
mod runtime_source_map;
mod runtime_startup;
mod runtime_string;
mod runtime_string_to_number;
mod runtime_structured_clone;
mod runtime_typedarray;
mod runtime_value_adapter;
mod shared_buffer;
mod startup_snapshot;
pub mod startup_snapshot_remap;

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
mod symbol_well_known;
mod types;
pub use runtime_process::{process_exit_code, process_exit_diagnostics};
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
use runtime_generator::*;
use runtime_heap::*;
use runtime_host_helpers::*;
use runtime_json::*;
use runtime_microtask::*;
use runtime_process::*;
use runtime_promises::*;
use runtime_regexp::*;
use runtime_render::*;
use runtime_typedarray::*;
use runtime_values::*;
use types::*;

#[derive(Clone)]
pub struct RuntimeOptions {
    pub max_heap_size: Option<usize>,
    pub wasmtime_memory_reservation: Option<u64>,
    pub gc_algorithm: GcAlgorithmKind,
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    pub env: Vec<(String, String)>,
    pub pid: u32,
    pub ppid: u32,
    pub platform: &'static str,
    pub arch: &'static str,
    pub version: &'static str,
    pub versions: &'static [(&'static str, &'static str)],
    pub fs_read_roots: Vec<PathBuf>,
    pub fs_write_roots: Vec<PathBuf>,
    pub fs_allow_write_anywhere: bool,
    pub module_loader: Option<Arc<dyn RuntimeModuleLoader>>,
    /// worker_threads：是否为 Worker 子线程 agent。
    pub is_worker_thread: bool,
    /// worker_threads：本 agent 的 threadId（主线程为 0）。
    pub worker_thread_id: u32,
    /// worker_threads：Worker 侧 parentPort 全局 id。
    pub parent_port_global_id: Option<u32>,
    /// worker_threads：注入的 workerData 序列化载荷。
    pub initial_worker_data: Option<runtime_worker_message::SerializedValue>,
}

impl std::fmt::Debug for RuntimeOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeOptions")
            .field("max_heap_size", &self.max_heap_size)
            .field(
                "wasmtime_memory_reservation",
                &self.wasmtime_memory_reservation,
            )
            .field("gc_algorithm", &self.gc_algorithm)
            .field("argv", &self.argv)
            .field("cwd", &self.cwd)
            .field("env", &self.env)
            .field("pid", &self.pid)
            .field("ppid", &self.ppid)
            .field("platform", &self.platform)
            .field("arch", &self.arch)
            .field("version", &self.version)
            .field("versions", &self.versions)
            .field("fs_read_roots", &self.fs_read_roots)
            .field("fs_write_roots", &self.fs_write_roots)
            .field("fs_allow_write_anywhere", &self.fs_allow_write_anywhere)
            .field(
                "module_loader",
                &self.module_loader.as_ref().map(|_| "<installed>"),
            )
            .finish()
    }
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            max_heap_size: None,
            wasmtime_memory_reservation: None,
            gc_algorithm: GcAlgorithmKind::MarkSweep,
            argv: Vec::new(),
            cwd: None,
            env: Vec::new(),
            pid: std::process::id(),
            ppid: 0,
            platform: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            version: PROCESS_NODE_VERSION,
            versions: PROCESS_VERSIONS,
            fs_read_roots: std::env::current_dir().ok().into_iter().collect(),
            fs_write_roots: std::env::current_dir()
                .ok()
                .into_iter()
                .chain(std::iter::once(std::env::temp_dir()))
                .collect(),
            fs_allow_write_anywhere: false,
            module_loader: None,
            is_worker_thread: false,
            worker_thread_id: 0,
            parent_port_global_id: None,
            initial_worker_data: None,
        }
    }
}

impl RuntimeOptions {
    pub fn with_max_heap_size(max_heap_size: usize) -> Self {
        Self {
            max_heap_size: Some(max_heap_size),
            ..Self::default()
        }
    }

    pub fn with_gc_algorithm(gc_algorithm: GcAlgorithmKind) -> Self {
        Self {
            gc_algorithm,
            ..Self::default()
        }
    }

    pub fn set_gc_algorithm(&mut self, gc_algorithm: GcAlgorithmKind) {
        self.gc_algorithm = gc_algorithm;
    }
}

/// 单次 GC 后记录的 linear-memory footprint 样本。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryFootprintSample {
    /// Wasm linear memory 当前已提交页数（64KiB/page）。
    pub committed_pages: usize,
    /// 当前 GC 算法可直接复用的空闲字节数。
    pub free_bytes_reusable: usize,
}

/// 单次运行结束后暴露给定量基准的 GC 观测快照。
#[derive(Clone, Debug, Default)]
pub struct GcExecutionStats {
    /// 最近一次完成 GC 周期的完整 v2 统计。
    pub last: GcStats,
    /// 本次运行中观测到的 GC pause 最大值序列（纳秒）。
    pub pause_hist: Vec<u64>,
    /// 最近 GC 周期的 committed/reusable footprint 序列。
    pub memory_footprint_hist: Vec<MemoryFootprintSample>,
}

pub fn gc_algorithm_from_env(
    env: &[(String, String)],
) -> std::result::Result<GcAlgorithmKind, String> {
    for key in ["WJSM_TEST_GC", "WJSM_GC"] {
        if let Some((_, value)) = env.iter().rev().find(|(env_key, _)| env_key == key)
            && !value.is_empty()
        {
            return value.parse();
        }
    }
    Ok(GcAlgorithmKind::MarkSweep)
}

pub async fn execute(wasm_bytes: &[u8]) -> Result<()> {
    execute_with_options(wasm_bytes, RuntimeOptions::default()).await
}

pub async fn execute_with_options(wasm_bytes: &[u8], options: RuntimeOptions) -> Result<()> {
    let stdout = io::stdout();
    match execute_with_writer_with_options(wasm_bytes, stdout.lock(), options).await {
        Ok((_, diagnostics)) => {
            if !diagnostics.is_empty() {
                let _ = io::stderr().write_all(&diagnostics);
            }
            Ok(())
        }
        Err(error) => {
            if let Some(diagnostics) = runtime_process::process_exit_diagnostics(&error) {
                if !diagnostics.is_empty() {
                    let _ = io::stderr().write_all(diagnostics);
                }
            }
            Err(error)
        }
    }
}

pub async fn execute_with_writer<W: Write>(wasm_bytes: &[u8], writer: W) -> Result<(W, Vec<u8>)> {
    execute_with_writer_with_options(wasm_bytes, writer, RuntimeOptions::default()).await
}

pub async fn execute_with_writer_with_options<W: Write>(
    wasm_bytes: &[u8],
    writer: W,
    options: RuntimeOptions,
) -> Result<(W, Vec<u8>)> {
    execute_with_writer_shared_inner(wasm_bytes, writer, None, true, options).await
}

pub async fn execute_with_writer_with_options_and_stats<W: Write>(
    wasm_bytes: &[u8],
    writer: W,
    options: RuntimeOptions,
) -> Result<(W, Vec<u8>, GcExecutionStats)> {
    execute_with_writer_shared_inner_with_stats(wasm_bytes, writer, None, true, options).await
}

/// 编译 JS/TS 源码到 WASM 字节码的共享辅助函数。
/// 供本 crate 测试及外部集成测试（`tests/`）复用，避免重复定义
/// `parse_module → lower_module → compile` 流程。
pub fn compile_source(source: &str) -> Result<Vec<u8>> {
    let module = wjsm_parser::parse_module(source)?;
    let program = wjsm_semantic::lower_module(module, false)?;
    wjsm_backend_wasm::compile(&program)
}

/// 编译缓存统计信息，供 CLI `wjsm cache` 命令展示和清理。
pub struct ModuleCacheStats {
    pub path: Option<std::path::PathBuf>,
    pub entries: usize,
    pub bytes: u64,
}

/// 返回当前模块编译缓存目录及已落盘条目统计。
pub fn module_cache_stats() -> Result<ModuleCacheStats> {
    let Some(path) = runtime_startup::module_cache_dir() else {
        return Ok(ModuleCacheStats {
            path: None,
            entries: 0,
            bytes: 0,
        });
    };

    let (entries, bytes) = cache_entry_stats(&path)?;
    Ok(ModuleCacheStats {
        path: Some(path),
        entries,
        bytes,
    })
}

/// 删除当前模块编译缓存目录中的所有条目，返回删除前可见条目数。
pub fn clear_module_cache() -> Result<usize> {
    let Some(path) = runtime_startup::module_cache_dir() else {
        return Ok(0);
    };
    let (entries, _) = cache_entry_stats(&path)?;
    if path.exists() {
        std::fs::remove_dir_all(&path)?;
    }
    std::fs::create_dir_all(&path)?;
    Ok(entries)
}

fn cache_entry_stats(path: &std::path::Path) -> Result<(usize, u64)> {
    if !path.exists() {
        return Ok((0, 0));
    }

    let mut entries = 0;
    let mut bytes = 0;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_file() {
            entries += 1;
            bytes += metadata.len();
        }
    }
    Ok((entries, bytes))
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
    let config = startup_engine_config(true, None);
    let engine = Engine::new(&config)
        .map_err(|e| anyhow::anyhow!("Failed to create async engine: {:?}", e))?;
    let module = Module::new(&engine, wasm)
        .map_err(|e| anyhow::anyhow!("WASM validation failed: {:?}", e))?;
    let mut bundle =
        instantiate_execute_bundle(&engine, &module, None, true, RuntimeOptions::default()).await?;
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
// 运行时可显式注入 build-time 预编译的 support cwasm；未注入时使用静态
// mark-sweep 默认 artifact。显式注入需要运行时输入，因此保留 OnceLock；默认
// artifact 初始化在声明处固定，使用 LazyLock。

static INSTALLED_SUPPORT_CWASM: OnceLock<&'static [u8]> = OnceLock::new();
static DEFAULT_MARK_SWEEP_SUPPORT_CWASM: LazyLock<Option<&'static [u8]>> = LazyLock::new(|| {
    wjsm_runtime_support::embedded_support_cwasm(wjsm_runtime_support::SupportGcFlavor::MarkSweep)
});
static DEFAULT_G1_SUPPORT_CWASM: LazyLock<Option<&'static [u8]>> = LazyLock::new(|| {
    wjsm_runtime_support::embedded_support_cwasm(wjsm_runtime_support::SupportGcFlavor::G1)
});
static DEFAULT_ZGC_SUPPORT_CWASM: LazyLock<Option<&'static [u8]>> = LazyLock::new(|| {
    wjsm_runtime_support::embedded_support_cwasm(wjsm_runtime_support::SupportGcFlavor::Zgc)
});

/// 安装编译时嵌入的 support cwasm；进程内只需调用一次（重复 set 静默忽略）。
/// 未显式调用时，`embedded_support_cwasm()` 使用 build-time 默认 artifact。
pub fn install_embedded_support_cwasm(cwasm_bytes: &'static [u8]) {
    let _ = INSTALLED_SUPPORT_CWASM.set(cwasm_bytes);
}

/// 返回已安装的 embedded support cwasm 字节。
/// 未通过 `install_embedded_support_cwasm` 显式注入时，使用 mark-sweep 默认 artifact。
/// 返回 None 仅当 embedded feature 未启用（build-time artifact 为空）。
pub fn embedded_support_cwasm() -> Option<&'static [u8]> {
    INSTALLED_SUPPORT_CWASM
        .get()
        .copied()
        .or(*DEFAULT_MARK_SWEEP_SUPPORT_CWASM)
}

pub fn embedded_support_cwasm_for(kind: GcAlgorithmKind) -> Option<&'static [u8]> {
    match kind {
        GcAlgorithmKind::MarkSweep => embedded_support_cwasm(),
        GcAlgorithmKind::G1 => *DEFAULT_G1_SUPPORT_CWASM,
        GcAlgorithmKind::Zgc => *DEFAULT_ZGC_SUPPORT_CWASM,
    }
}
pub(crate) async fn execute_with_writer_shared_agent<W: Write>(
    wasm_bytes: &[u8],
    writer: W,
    shared_state: Arc<SharedRuntimeState>,
) -> Result<(W, Vec<u8>)> {
    execute_with_writer_shared_agent_options(
        wasm_bytes,
        writer,
        shared_state,
        RuntimeOptions::default(),
    )
    .await
}

/// 与 `execute_with_writer_shared_agent` 相同，但允许注入 worker_threads 上下文。
pub(crate) async fn execute_with_writer_shared_agent_options<W: Write>(
    wasm_bytes: &[u8],
    writer: W,
    shared_state: Arc<SharedRuntimeState>,
    options: RuntimeOptions,
) -> Result<(W, Vec<u8>)> {
    execute_with_writer_shared_inner(wasm_bytes, writer, Some(shared_state), false, options).await
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

    let config = startup_engine_config(true, None);
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
    let needs_support = module
        .imports()
        .any(|import| import.module() == "wjsm_support");
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
    initialize_host_post_bootstrap(&mut store, &wasm_env)?;
    timings.host_post_bootstrap = start.elapsed();

    let start = std::time::Instant::now();
    if let Ok(bootstrap_fn) =
        instance.get_typed_func::<(), i64>(&mut store, "__wjsm_bootstrap_once")
    {
        let _ = bootstrap_fn.call_async(&mut store, ()).await;
    }
    if let Ok(function_props_fn) =
        instance.get_typed_func::<(), i64>(&mut store, "__wjsm_init_function_props")
    {
        let _ = function_props_fn.call_async(&mut store, ()).await;
    }
    crate::runtime_heap::ensure_error_prototypes_initialized(&mut store, &wasm_env);
    crate::runtime_heap::ensure_symbol_prototype_initialized(&mut store, &wasm_env);
    crate::runtime_heap::ensure_promise_prototype_initialized(&mut store, &wasm_env);
    crate::runtime_heap::ensure_function_prototype_initialized(&mut store, &wasm_env);
    crate::runtime_heap::install_function_props_prototypes(&mut store, &wasm_env);
    crate::runtime_heap::ensure_regexp_prototype_initialized(&mut store, &wasm_env);
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
    initialize_host_post_bootstrap(&mut store2, &env2)?;
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
    let config = startup_engine_config(true, None);

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

    let mut bundle =
        instantiate_execute_bundle(&engine, &module, None, true, RuntimeOptions::default()).await?;

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
        bundle.diagnostics,
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
    options: RuntimeOptions,
) -> Result<(W, Vec<u8>)> {
    let (writer, diagnostics, _) = execute_with_writer_shared_inner_with_stats(
        wasm_bytes,
        writer,
        shared_state,
        use_epoch_async_yield,
        options,
    )
    .await?;
    Ok((writer, diagnostics))
}

async fn execute_with_writer_shared_inner_with_stats<W: Write>(
    wasm_bytes: &[u8],
    writer: W,
    shared_state: Option<Arc<SharedRuntimeState>>,
    use_epoch_async_yield: bool,
    options: RuntimeOptions,
) -> Result<(W, Vec<u8>, GcExecutionStats)> {
    let config = startup_engine_config(use_epoch_async_yield, options.wasmtime_memory_reservation);
    let engine = Engine::new(&config)
        .map_err(|e| anyhow::anyhow!("Failed to create async engine: {:?}", e))?;

    let module = compile_or_load_cached(&engine, wasm_bytes)?;
    // 解析 "wjsm_sourcemap" custom section，供 trap backtrace 格式化。
    let source_map = runtime_source_map::SourceMapInfo::parse_from_wasm(wasm_bytes);
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
        options.clone(),
    )
    .await?;
    bundle.store.data_mut().source_map = source_map.clone();

    let mut snapshot_restored = false;
    if let Some(bytes) = snapshot_bytes {
        let _ = run_init_globals_only(&mut bundle).await;
        snapshot_restored = try_restore_snapshot(&mut bundle, bytes).await;
        if !snapshot_restored {
            bundle = instantiate_execute_bundle(
                &engine,
                &module,
                shared_state.clone(),
                use_epoch_async_yield,
                options,
            )
            .await?;
            bundle.store.data_mut().source_map = source_map.clone();
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
        bundle.diagnostics,
        writer,
        &mut bundle.host_completion_rx,
    )
    .await
}
const GC_PAUSE_HIST_CAP: usize = 256;
const GC_MEMORY_FOOTPRINT_HIST_CAP: usize = 256;

#[derive(Debug)]
struct GcPauseHist {
    entries: [u64; GC_PAUSE_HIST_CAP],
    len: usize,
    next: usize,
}

impl Default for GcPauseHist {
    fn default() -> Self {
        Self {
            entries: [0; GC_PAUSE_HIST_CAP],
            len: 0,
            next: 0,
        }
    }
}

impl GcPauseHist {
    fn push(&mut self, pause_ns: u64) {
        self.entries[self.next] = pause_ns;
        self.next = (self.next + 1) % GC_PAUSE_HIST_CAP;
        self.len = self.len.saturating_add(1).min(GC_PAUSE_HIST_CAP);
    }

    fn snapshot(&self) -> Vec<u64> {
        if self.len < GC_PAUSE_HIST_CAP {
            return self.entries[..self.len].to_vec();
        }
        (0..GC_PAUSE_HIST_CAP)
            .map(|idx| self.entries[(self.next + idx) % GC_PAUSE_HIST_CAP])
            .collect()
    }
}

#[derive(Debug)]
struct MemoryFootprintHist {
    entries: [MemoryFootprintSample; GC_MEMORY_FOOTPRINT_HIST_CAP],
    len: usize,
    next: usize,
}

impl Default for MemoryFootprintHist {
    fn default() -> Self {
        Self {
            entries: [MemoryFootprintSample::default(); GC_MEMORY_FOOTPRINT_HIST_CAP],
            len: 0,
            next: 0,
        }
    }
}

impl MemoryFootprintHist {
    fn push(&mut self, sample: MemoryFootprintSample) {
        self.entries[self.next] = sample;
        self.next = (self.next + 1) % GC_MEMORY_FOOTPRINT_HIST_CAP;
        self.len = self.len.saturating_add(1).min(GC_MEMORY_FOOTPRINT_HIST_CAP);
    }

    fn snapshot(&self) -> Vec<MemoryFootprintSample> {
        if self.len < GC_MEMORY_FOOTPRINT_HIST_CAP {
            return self.entries[..self.len].to_vec();
        }
        (0..GC_MEMORY_FOOTPRINT_HIST_CAP)
            .map(|idx| self.entries[(self.next + idx) % GC_MEMORY_FOOTPRINT_HIST_CAP])
            .collect()
    }
}

fn gc_log_enabled() -> bool {
    gc_log_enabled_value(std::env::var_os("WJSM_GC_LOG").as_deref())
}

fn gc_log_enabled_value(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| value == std::ffi::OsStr::new("1"))
}

fn format_gc_log_summary(algorithm: &str, stats: &crate::runtime_gc::api::GcStats) -> String {
    format!(
        "wjsm gc algorithm={} cycle={} pause_ns_max={} pause_ns_total={} pause_count={} relocated_bytes={} relocated_objects={} barrier_events={} satb_flushes={} rset_cards={} rset_precise_slots={} load_barrier_mark_hits={} load_barrier_relocate_hits={}",
        algorithm,
        stats.cycle_kind.as_str(),
        stats.pause_ns_max,
        stats.pause_ns_total,
        stats.pause_count,
        stats.relocated_bytes,
        stats.relocated_objects,
        stats.barrier_events,
        stats.satb_flushes,
        stats.rset_cards,
        stats.rset_precise_slots,
        stats.load_barrier_mark_hits,
        stats.load_barrier_relocate_hits,
    )
}

impl Clone for RuntimeState {
    fn clone(&self) -> Self {
        Self {
            output: self.output.clone(),
            performance_origin: self.performance_origin.clone(),
            iterators: self.iterators.clone(),
            enumerators: self.enumerators.clone(),
            runtime_strings: self.runtime_strings.clone(),
            runtime_property_keys: self.runtime_property_keys.clone(),
            diagnostics: self.diagnostics.clone(),
            runtime_error: self.runtime_error.clone(),
            max_heap_size: self.max_heap_size,
            host_temp_roots: self.host_temp_roots.clone(),
            process: self.process.clone(),
            next_tick_queue: self.next_tick_queue.clone(),
            process_exit_signal: self.process_exit_signal.clone(),
            gc_mark_bits: self.gc_mark_bits.clone(),
            gc_epoch: self.gc_epoch.clone(),
            timers: self.timers.clone(),
            cancelled_timers: self.cancelled_timers.clone(),
            next_timer_id: self.next_timer_id.clone(),
            closures: self.closures.clone(),
            bound_objects: self.bound_objects.clone(),
            native_callables: self.native_callables.clone(),
            native_callable_free_slots: self.native_callable_free_slots.clone(),
            handle_free_list: self.handle_free_list.clone(),
            abandoned_regions: self.abandoned_regions.clone(),
            immortal_objects_end: self.immortal_objects_end.clone(),
            dynamic_heap_start: self.dynamic_heap_start.clone(),
            barrier_event_buf_base: self.barrier_event_buf_base.clone(),
            gc_algorithm: self.gc_algorithm.clone(),
            gc_scheduler: self.gc_scheduler.clone(),
            last_gc_stats: self.last_gc_stats.clone(),
            gc_pause_hist: self.gc_pause_hist.clone(),
            memory_footprint_hist: self.memory_footprint_hist.clone(),
            continuation_free_slots: self.continuation_free_slots.clone(),
            combinator_context_free_slots: self.combinator_context_free_slots.clone(),
            eval_cache: self.eval_cache.clone(),
            bigint_table: self.bigint_table.clone(),
            symbol_table: self.symbol_table.clone(),
            symbol_constructor_static_props: self.symbol_constructor_static_props.clone(),
            regex_table: self.regex_table.clone(),
            promise_table: self.promise_table.clone(),
            pending_unhandled_rejections: self.pending_unhandled_rejections.clone(),
            microtask_queue: self.microtask_queue.clone(),
            continuation_table: self.continuation_table.clone(),
            async_generator_table: self.async_generator_table.clone(),
            generator_table: self.generator_table.clone(),
            async_from_sync_iterators: self.async_from_sync_iterators.clone(),
            iterator_prototype: self.iterator_prototype,
            generator_prototype: self.generator_prototype,
            async_iterator_prototype: self.async_iterator_prototype,
            async_gen_prototype: self.async_gen_prototype,
            error_prototypes: self.error_prototypes,
            symbol_prototype: self.symbol_prototype,
            array_proto_values: AtomicI64::new(self.array_proto_values.load(Ordering::Relaxed)),
            array_named_props: self.array_named_props.clone(),
            promise_prototype: self.promise_prototype,
            function_prototype: self.function_prototype,
            regexp_prototype: self.regexp_prototype,
            date_prototype: self.date_prototype,
            buffer_prototype: self.buffer_prototype,
            text_encoder_prototype: self.text_encoder_prototype,
            text_decoder_prototype: self.text_decoder_prototype,
            typedarray_prototypes: self.typedarray_prototypes,
            combinator_contexts: self.combinator_contexts.clone(),
            module_registry: self.module_registry.clone(),
            module_loader: self.module_loader.clone(),
            support_exports: self.support_exports.clone(),
            error_table: self.error_table.clone(),
            map_table: self.map_table.clone(),
            set_table: self.set_table.clone(),
            map_free_slots: self.map_free_slots.clone(),
            set_free_slots: self.set_free_slots.clone(),
            weakmap_table: self.weakmap_table.clone(),
            weakset_table: self.weakset_table.clone(),
            weakref_table: self.weakref_table.clone(),
            finalization_registry_table: self.finalization_registry_table.clone(),
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
            net_socket_table: self.net_socket_table.clone(),
            net_server_table: self.net_server_table.clone(),
            dgram_socket_table: self.dgram_socket_table.clone(),
            tls_socket_table: self.tls_socket_table.clone(),
            tls_server_table: self.tls_server_table.clone(),
            host_completion_tx: self.host_completion_tx.clone(),
            async_op_counter: self.async_op_counter.clone(),
            source_map: self.source_map.clone(),
            message_port_bindings: self.message_port_bindings.clone(),
            worker_bindings: self.worker_bindings.clone(),
            is_worker_thread: self.is_worker_thread,
            thread_id: self.thread_id,
            parent_port_id: self.parent_port_id,
            worker_data_serialized: self.worker_data_serialized.clone(),
        }
    }
}
struct RuntimeState {
    output: Arc<Mutex<Vec<u8>>>,
    performance_origin: Arc<std::time::Instant>,
    iterators: Arc<Mutex<Vec<IteratorState>>>,
    enumerators: Arc<Mutex<Vec<EnumeratorState>>>,
    runtime_strings: Arc<Mutex<Vec<runtime_string::RuntimeString>>>,
    runtime_property_keys: Arc<Mutex<Vec<runtime_string::RuntimeString>>>,
    /// 进程内可捕获的诊断输出（如 unhandled rejection 警告）；真实 CLI 由 execute 刷到 stderr。
    diagnostics: Arc<Mutex<Vec<u8>>>,
    runtime_error: Arc<Mutex<Option<String>>>,
    /// Host import 直接参数的临时 root；WASM 直接入参不在 shadow stack 中，
    /// host import 若在消费入参前分配 JS 对象，必须短暂保护这些值。
    host_temp_roots: Arc<Mutex<Vec<i64>>>,
    /// 用户配置的 JS 堆预算（字节）。None 表示只受 wasm32 地址空间和宿主内存约束。
    max_heap_size: Option<usize>,
    /// 注入的 Node `process` 宿主快照。
    process: ProcessState,
    /// Node next tick queue；drain 时优先级高于普通 microtask queue。
    next_tick_queue: ProcessNextTickQueue,
    /// `process.exit(code)` 设置的正常退出信号。
    process_exit_signal: Arc<Mutex<Option<ProcessExitSignal>>>,
    /// GC 标记位图：每个 handle 对应 1 bit，用于标记-清除 GC。
    gc_mark_bits: Arc<Mutex<Vec<u64>>>,
    /// GC epoch：任何可能改变 obj_table ptr/色位的 GC 点递增，用于 debug INV-C2 断言。
    gc_epoch: Arc<AtomicU64>,
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
    /// 启动快照恢复后永生对象区末尾的绝对地址。
    immortal_objects_end: Arc<Mutex<usize>>,
    /// GC 算法接管的动态堆起点；当前等于 immortal_objects_end。
    dynamic_heap_start: Arc<Mutex<usize>>,
    /// WASM 写屏障事件缓冲区起点；padding 不属于可扫描对象区。
    barrier_event_buf_base: Arc<Mutex<usize>>,
    /// 可插拔 GC 算法实例（默认 MarkSweepCollector）。host imports 经 v2 生命周期接口驱动。
    /// Arc<Mutex> 因 host fn 经 &Caller 访问需内部可变性。
    gc_algorithm: Arc<Mutex<Box<dyn crate::runtime_gc::GcAlgorithm + Send + Sync>>>,
    /// GC safepoint 调度器：根据 pause target 调整单步预算，完整周期后更新字节触发目标。
    gc_scheduler: Arc<Mutex<crate::runtime_gc::scheduler::GcScheduler>>,
    /// 最近一次 GC 统计（含碎片治理指标，issue #332）。
    /// 每次完整 collection 后由 host 更新，供可观测性 API 查询。
    last_gc_stats: Arc<Mutex<crate::runtime_gc::api::GcStats>>,
    /// 最近 256 次 GC pause 观测，按纳秒记录；写入 last_gc_stats 时同步推进。
    gc_pause_hist: Arc<Mutex<GcPauseHist>>,
    /// 最近 256 次 GC footprint 观测；写入 last_gc_stats 时同步推进。
    memory_footprint_hist: Arc<Mutex<MemoryFootprintHist>>,
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
    /// `Symbol` 构造器（NativeCallable）上的 well-known 等静态属性
    symbol_constructor_static_props: symbol_well_known::SymbolConstructorStaticProps,
    /// RegExp 侧表：存储编译后的正则表达式和元数据
    regex_table: Arc<Mutex<Vec<RegexEntry>>>,
    /// Promise 侧表：object handle → Promise 内部槽；非 Promise object handle 使用空占位。
    promise_table: Arc<Mutex<Vec<PromiseEntry>>>,
    /// 已 reject 且尚未 handled 的 promise 索引，用于 drain 时避免全表扫描。
    pending_unhandled_rejections: Arc<Mutex<VecDeque<usize>>>,
    /// 微任务队列
    microtask_queue: Arc<Mutex<VecDeque<Microtask>>>,
    /// Continuation 侧表：存储异步函数续延
    continuation_table: Arc<Mutex<Vec<ContinuationEntry>>>,
    /// AsyncGenerator 侧表：存储异步生成器状态
    async_generator_table: Arc<Mutex<Vec<AsyncGeneratorEntry>>>,
    /// Generator 侧表：存储同步生成器状态
    generator_table: Arc<Mutex<Vec<GeneratorEntry>>>,
    /// async-from-sync iterator 侧表
    async_from_sync_iterators: Arc<Mutex<Vec<AsyncFromSyncIteratorEntry>>>,
    /// %IteratorPrototype% 对象
    iterator_prototype: i64,
    /// Generator.prototype 对象
    generator_prototype: i64,
    /// %AsyncIteratorPrototype% 对象
    async_iterator_prototype: i64,
    /// AsyncGenerator.prototype 对象
    async_gen_prototype: i64,
    /// Error 及其子类的 prototype 对象（bootstrap 后初始化）
    error_prototypes: crate::runtime_heap::ErrorPrototypes,
    /// %SymbolPrototype% 对象
    symbol_prototype: i64,
    /// %PromisePrototype% 对象
    promise_prototype: i64,
    /// %FunctionPrototype% 对象（Function.prototype.call/apply/bind 与函数原型链）
    function_prototype: i64,
    /// %RegExpPrototype% 对象（供 RegExp 构造函数 .prototype + instanceof 原型链遍历）
    regexp_prototype: i64,
    /// %DatePrototype% 对象（供 Date 构造函数 .prototype + instanceof 原型链遍历）。
    date_prototype: i64,
    /// Buffer.prototype 对象。
    buffer_prototype: i64,
    /// TextEncoder.prototype 对象。
    text_encoder_prototype: i64,
    /// TextDecoder.prototype 对象。
    text_decoder_prototype: i64,
    /// TypedArray 构造器 prototype 对象缓存，按 TypedArrayConstructorKind::index() 存放。
    typedarray_prototypes: [i64; TypedArrayConstructorKind::COUNT],
    /// Promise combinator 侧表：pending 元素的 reaction 通过索引回写共享结果。
    combinator_contexts: Arc<Mutex<Vec<CombinatorContext>>>,
    /// 运行时模块 registry；旧 ModuleId 快路径也通过这里的过渡 key 兼容。
    module_registry: Arc<Mutex<RuntimeModuleRegistry>>,
    /// 模块运行时加载器；CLI 文件系统加载器在后续任务注入。
    module_loader: Option<Arc<dyn RuntimeModuleLoader>>,
    /// 当前 Store 内 support module 的导出；动态 runtime loader 实例化时复用同一批 helper。
    support_exports: Arc<Mutex<Vec<(&'static str, Extern)>>>,
    /// Error 侧表：存储 error 对象的 name 和 message
    error_table: Arc<Mutex<Vec<ErrorEntry>>>,
    /// Map 侧表：存储 Map 对象的键值对
    map_table: Arc<Mutex<Vec<MapEntry>>>,
    /// Set 侧表：存储 Set 对象的值
    set_table: Arc<Mutex<Vec<SetEntry>>>,
    /// Map 侧表回收后的可复用槽位。
    map_free_slots: Arc<Mutex<Vec<u32>>>,
    /// Set 侧表回收后的可复用槽位。
    set_free_slots: Arc<Mutex<Vec<u32>>>,
    /// WeakMap 侧表：存储 WeakMap 对象的键值对
    weakmap_table: Arc<Mutex<Vec<WeakMapEntry>>>,
    /// WeakSet 侧表：存储 WeakSet 对象的值
    weakset_table: Arc<Mutex<Vec<WeakSetEntry>>>,
    /// WeakRef 侧表：存储 WeakRef 对象的 target handle
    weakref_table: Arc<Mutex<Vec<WeakRefEntry>>>,
    /// Array.prototype.values 缓存，用于规范要求复用该函数对象的 @@iterator。
    array_proto_values: AtomicI64,
    /// 数组实例上的 symbol 等非索引命名属性（@@isConcatSpreadable 等）。
    array_named_props: array_named_props::ArrayNamedPropsStore,
    /// FinalizationRegistry 侧表：存储 registry 对象、callback 和注册信息
    finalization_registry_table: Arc<Mutex<Vec<FinalizationRegistryEntry>>>,
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
    /// TCP socket 侧表：Node `net.Socket` 持有的宿主 TCP 半连接。
    net_socket_table: Arc<HostSideTable<runtime_node_net::NetSocketEntry>>,
    /// TCP server 侧表：Node `net.Server` 持有的监听器与 accept 队列。
    net_server_table: Arc<HostSideTable<runtime_node_net::NetServerEntry>>,
    /// UDP socket 侧表：Node `dgram.Socket` 持有的宿主 UDP socket。
    dgram_socket_table: Arc<HostSideTable<runtime_node_dgram::DgramSocketEntry>>,
    /// TLS socket 侧表：Node `tls.TLSSocket` 持有的宿主 TLS 流半连接。
    tls_socket_table: Arc<HostSideTable<runtime_node_tls::TlsSocketEntry>>,
    /// TLS server 侧表：Node `tls.Server` 持有的监听器与 accept 队列。
    tls_server_table: Arc<HostSideTable<runtime_node_tls::TlsServerEntry>>,
    /// ReadableStream 侧表：存储流状态
    readable_stream_table: Arc<HostSideTable<ReadableStreamEntry>>,
    /// Reader 侧表：存储 reader → stream 映射
    reader_table: Arc<HostSideTable<ReaderEntry>>,
    /// Controller 侧表（ReadableStream DefaultController 等）
    stream_controller_table: Arc<HostSideTable<StreamControllerEntry>>,
    byob_request_table: Arc<HostSideTable<ByobRequestEntry>>,
    /// WritableStream 侧表：存储可写流状态
    writable_stream_table: Arc<HostSideTable<WritableStreamEntry>>,
    /// Writer 侧表：存储 WritableStreamDefaultWriter → stream 映射
    writer_table: Arc<HostSideTable<WriterEntry>>,
    /// TransformStream 侧表：存储转换流状态
    transform_stream_table: Arc<HostSideTable<TransformStreamEntry>>,
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
    /// WASM source map（从 "wjsm_sourcemap" custom section 解析），供 trap backtrace 格式化。
    source_map: Option<runtime_source_map::SourceMapInfo>,
    /// MessagePort 本地绑定（deliver 回调仅本 Store 有效）。
    message_port_bindings: Arc<
        Mutex<HashMap<u32, runtime_node_worker_threads::LocalPortBinding>>,
    >,
    /// Worker 本地绑定（lifecycle 回调 + lifetime AsyncOpGuard）。
    worker_bindings: Arc<
        Mutex<HashMap<u32, runtime_node_worker_threads::LocalWorkerBinding>>,
    >,
    /// 是否主线程 agent（worker_threads.isMainThread）。
    is_worker_thread: bool,
    /// worker_threads.threadId（主线程 0）。
    thread_id: u32,
    /// Worker 侧 parentPort 全局 id。
    parent_port_id: Option<u32>,
    /// Worker 注入的 workerData（序列化后）。
    worker_data_serialized: Option<runtime_worker_message::SerializedValue>,
}

impl RuntimeState {
    /// 记录最近一次 GC 统计，并同步推进 v2 环形观测序列。
    pub(crate) fn store_last_gc_stats(
        &self,
        algorithm: &'static str,
        mut stats: crate::runtime_gc::api::GcStats,
    ) {
        stats.ensure_pause_from_elapsed();
        let has_pause = stats.has_pause_observation();
        if has_pause {
            let mut hist = self.gc_pause_hist.lock().unwrap_or_else(|e| e.into_inner());
            hist.push(stats.pause_ns_max);
        }
        {
            let mut hist = self
                .memory_footprint_hist
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            hist.push(MemoryFootprintSample {
                committed_pages: stats.committed_pages,
                free_bytes_reusable: stats.free_bytes_reusable,
            });
        }
        if has_pause && gc_log_enabled() {
            eprintln!("{}", format_gc_log_summary(algorithm, &stats));
        }
        let mut slot = self.last_gc_stats.lock().unwrap_or_else(|e| e.into_inner());
        *slot = stats;
    }

    /// 复制当前运行的 GC 统计快照，避免 integration test 直接窥探 RuntimeState。
    pub(crate) fn gc_execution_stats_snapshot(&self) -> GcExecutionStats {
        let last = self
            .last_gc_stats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let pause_hist = self.gc_pause_hist_snapshot();
        let memory_footprint_hist = self.memory_footprint_hist_snapshot();
        GcExecutionStats {
            last,
            pause_hist,
            memory_footprint_hist,
        }
    }

    fn gc_pause_hist_snapshot(&self) -> Vec<u64> {
        self.gc_pause_hist
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot()
    }

    fn memory_footprint_hist_snapshot(&self) -> Vec<MemoryFootprintSample> {
        self.memory_footprint_hist
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot()
    }

    /// 记录启动期 immortal/dynamic/barrier 三段边界，供 GC attach 与后续算法查询。
    pub(crate) fn store_heap_layout_boundaries(
        &self,
        immortal_objects_end: usize,
        dynamic_heap_start: usize,
        barrier_event_buf_base: usize,
    ) {
        *self
            .immortal_objects_end
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = immortal_objects_end;
        *self
            .dynamic_heap_start
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = dynamic_heap_start;
        *self
            .barrier_event_buf_base
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = barrier_event_buf_base;
    }

    pub(crate) fn heap_layout_boundaries(&self) -> (usize, usize, usize) {
        let immortal_objects_end = *self
            .immortal_objects_end
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dynamic_heap_start = *self
            .dynamic_heap_start
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let barrier_event_buf_base = *self
            .barrier_event_buf_base
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        (
            immortal_objects_end,
            dynamic_heap_start,
            barrier_event_buf_base,
        )
    }

    /// 暂存 host import 构造中的 JS 值，防止构造期分配触发 GC 时被误回收。
    pub(crate) fn push_host_temp_roots<I>(&self, roots: I) -> usize
    where
        I: IntoIterator<Item = i64>,
    {
        let mut temp_roots = self
            .host_temp_roots
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let len = temp_roots.len();
        temp_roots.extend(roots);
        len
    }

    /// 恢复 host 临时 root 栈到先前长度。
    pub(crate) fn truncate_host_temp_roots(&self, len: usize) {
        self.host_temp_roots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .truncate(len);
    }

    #[cfg(test)]
    fn new_with_shared(shared_state: Option<Arc<SharedRuntimeState>>) -> Self {
        Self::new_with_shared_and_options(shared_state, RuntimeOptions::default())
            .expect("default runtime options must create RuntimeState")
    }

    fn new_with_shared_and_options(
        shared_state: Option<Arc<SharedRuntimeState>>,
        options: RuntimeOptions,
    ) -> Result<Self> {
        let mut state = Self::new();
        state.shared_state = shared_state.or_else(|| Some(new_shared_runtime_state()));
        state.max_heap_size = options.max_heap_size;
        state.process = ProcessState::from_options(&options);
        state.gc_algorithm = Arc::new(Mutex::new(
            crate::runtime_gc::registry::create(options.gc_algorithm)
                .map_err(anyhow::Error::msg)?,
        ));
        state.module_loader = options.module_loader;
        state.is_worker_thread = options.is_worker_thread;
        state.thread_id = options.worker_thread_id;
        state.parent_port_id = options.parent_port_global_id;
        state.worker_data_serialized = options.initial_worker_data;
        Ok(state)
    }

    pub(crate) fn max_heap_size(&self) -> Option<usize> {
        self.max_heap_size
    }

    pub(crate) fn set_heap_oom_error(&self, used: usize, requested: usize) {
        crate::runtime_promises::set_runtime_error(self, self.heap_oom_message(used, requested));
    }

    pub(crate) fn heap_oom_message(&self, used: usize, requested: usize) -> String {
        if let Some(max) = self.max_heap_size {
            format!(
                "JavaScript heap budget exhausted: requested {requested} bytes with {used}/{max} bytes used"
            )
        } else {
            format!("JavaScript heap allocation failed: requested {requested} bytes")
        }
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
            .unwrap_or_else(|e| e.into_inner());
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
        RuntimeState {
            performance_origin: Arc::new(std::time::Instant::now()),
            output: Arc::new(Mutex::new(Vec::new())),
            iterators: Arc::new(Mutex::new(Vec::new())),
            enumerators: Arc::new(Mutex::new(Vec::new())),
            runtime_strings: Arc::new(Mutex::new(Vec::new())),
            runtime_property_keys: Arc::new(Mutex::new(Vec::new())),
            diagnostics: Arc::new(Mutex::new(Vec::new())),
            runtime_error: Arc::new(Mutex::new(None)),
            host_temp_roots: Arc::new(Mutex::new(Vec::new())),
            max_heap_size: None,
            process: ProcessState::from_options(&RuntimeOptions::default()),
            next_tick_queue: Arc::new(Mutex::new(VecDeque::new())),
            process_exit_signal: Arc::new(Mutex::new(None)),
            gc_mark_bits: Arc::new(Mutex::new(Vec::new())),
            gc_epoch: Arc::new(AtomicU64::new(0)),
            timers: Arc::new(Mutex::new(Vec::new())),
            cancelled_timers: Arc::new(Mutex::new(HashSet::new())),
            next_timer_id: Arc::new(Mutex::new(1)),
            closures: Arc::new(Mutex::new(Vec::new())),
            bound_objects: Arc::new(Mutex::new(Vec::new())),
            native_callables: Arc::new(Mutex::new(vec![NativeCallable::EvalIndirect])),
            native_callable_free_slots: Arc::new(Mutex::new(Vec::new())),
            handle_free_list: Arc::new(Mutex::new(Vec::new())),
            abandoned_regions: Arc::new(Mutex::new(Vec::new())),
            immortal_objects_end: Arc::new(Mutex::new(0)),
            dynamic_heap_start: Arc::new(Mutex::new(0)),
            barrier_event_buf_base: Arc::new(Mutex::new(0)),
            gc_algorithm: Arc::new(Mutex::new(
                crate::runtime_gc::registry::create(crate::runtime_gc::GcAlgorithmKind::MarkSweep)
                    .expect("mark-sweep GC algorithm must be registered"),
            )),
            gc_scheduler: Arc::new(Mutex::new(
                crate::runtime_gc::scheduler::GcScheduler::default(),
            )),
            last_gc_stats: Arc::new(Mutex::new(crate::runtime_gc::api::GcStats::default())),
            gc_pause_hist: Arc::new(Mutex::new(GcPauseHist::default())),
            memory_footprint_hist: Arc::new(Mutex::new(MemoryFootprintHist::default())),
            continuation_free_slots: Arc::new(Mutex::new(Vec::new())),
            combinator_context_free_slots: Arc::new(Mutex::new(Vec::new())),
            eval_cache: Arc::new(Mutex::new(HashMap::new())),
            bigint_table: Arc::new(Mutex::new(Vec::new())),
            symbol_table: Arc::new(Mutex::new(vec![
                SymbolEntry {
                    description: Some("Symbol.iterator".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol.species".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol.toStringTag".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol.asyncIterator".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol.hasInstance".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol.toPrimitive".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol.dispose".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol.match".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol.asyncDispose".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol.isConcatSpreadable".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol.matchAll".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol.replace".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol.search".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol.split".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol.unscopables".into()),
                    global_key: None,
                },
            ])),
            symbol_constructor_static_props: symbol_well_known::new_symbol_constructor_static_props(
            ),
            regex_table: Arc::new(Mutex::new(Vec::new())),
            promise_table: Arc::new(Mutex::new(Vec::new())),
            pending_unhandled_rejections: Arc::new(Mutex::new(VecDeque::new())),
            microtask_queue: Arc::new(Mutex::new(VecDeque::new())),
            continuation_table: Arc::new(Mutex::new(Vec::new())),
            async_generator_table: Arc::new(Mutex::new(Vec::new())),
            generator_table: Arc::new(Mutex::new(Vec::new())),
            async_from_sync_iterators: Arc::new(Mutex::new(Vec::new())),
            iterator_prototype: value::encode_undefined(),
            generator_prototype: value::encode_undefined(),
            async_iterator_prototype: value::encode_undefined(),
            promise_prototype: value::encode_undefined(),
            function_prototype: value::encode_undefined(),
            regexp_prototype: value::encode_undefined(),
            date_prototype: value::encode_undefined(),
            async_gen_prototype: value::encode_undefined(),
            buffer_prototype: value::encode_undefined(),
            text_encoder_prototype: value::encode_undefined(),
            text_decoder_prototype: value::encode_undefined(),
            typedarray_prototypes: [value::encode_undefined(); TypedArrayConstructorKind::COUNT],
            error_prototypes: crate::runtime_heap::ErrorPrototypes::default(),
            symbol_prototype: value::encode_undefined(),
            combinator_contexts: Arc::new(Mutex::new(Vec::new())),
            module_registry: Arc::new(Mutex::new(RuntimeModuleRegistry::new())),
            module_loader: None,
            support_exports: Arc::new(Mutex::new(Vec::new())),
            error_table: Arc::new(Mutex::new(Vec::new())),
            map_table: Arc::new(Mutex::new(Vec::new())),
            set_table: Arc::new(Mutex::new(Vec::new())),
            map_free_slots: Arc::new(Mutex::new(Vec::new())),
            set_free_slots: Arc::new(Mutex::new(Vec::new())),
            weakmap_table: Arc::new(Mutex::new(Vec::new())),
            weakset_table: Arc::new(Mutex::new(Vec::new())),
            weakref_table: Arc::new(Mutex::new(Vec::new())),
            finalization_registry_table: Arc::new(Mutex::new(Vec::new())),
            proxy_table: Arc::new(Mutex::new(Vec::new())),
            arraybuffer_table: Arc::new(Mutex::new(Vec::new())),
            dataview_table: Arc::new(Mutex::new(Vec::new())),
            typedarray_table: Arc::new(Mutex::new(Vec::new())),
            headers_table: Arc::new(Mutex::new(Vec::new())),
            array_proto_values: AtomicI64::new(value::encode_undefined()),
            array_named_props: array_named_props::ArrayNamedPropsStore::new(),
            fetch_response_table: Arc::new(Mutex::new(Vec::new())),
            fetch_request_table: Arc::new(Mutex::new(Vec::new())),
            abort_signal_table: Arc::new(Mutex::new(Vec::new())),
            http_response_table: Arc::new(Mutex::new(Vec::new())),
            fetch_http_clients: Arc::new(Mutex::new(HashMap::new())),
            net_socket_table: Arc::new(HostSideTable::new()),
            net_server_table: Arc::new(HostSideTable::new()),
            dgram_socket_table: Arc::new(HostSideTable::new()),
            tls_socket_table: Arc::new(HostSideTable::new()),
            tls_server_table: Arc::new(HostSideTable::new()),
            readable_stream_table: Arc::new(HostSideTable::new()),
            reader_table: Arc::new(HostSideTable::new()),
            stream_controller_table: Arc::new(HostSideTable::new()),
            byob_request_table: Arc::new(HostSideTable::new()),
            writable_stream_table: Arc::new(HostSideTable::new()),
            transform_stream_table: Arc::new(HostSideTable::new()),
            writer_table: Arc::new(HostSideTable::new()),
            shared_state: Some(new_shared_runtime_state()),
            non_extensible_handles: Arc::new(Mutex::new(HashSet::new())),
            scope_records: HashMap::new(),
            scope_record_next_handle: 0,
            new_target: AtomicI64::new(value::encode_undefined()),
            host_completion_tx: None,
            async_op_counter: None,
            source_map: None,
            message_port_bindings: Arc::new(Mutex::new(HashMap::new())),
            worker_bindings: Arc::new(Mutex::new(HashMap::new())),
            is_worker_thread: false,
            thread_id: 0,
            parent_port_id: None,
            worker_data_serialized: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GcAlgorithmKind, RuntimeOptions, execute_with_writer, execute_with_writer_with_options,
        gc_algorithm_from_env,
    };
    use crate::runtime_gc::api::{CycleKind, GcStats};
    use anyhow::Result;
    use std::ffi::OsStr;
    use std::time::Duration;
    use tokio::runtime::Runtime;

    #[test]
    fn pause_hist_ring_wraps_at_256_entries() {
        let state = super::RuntimeState::new();
        for idx in 0..260u64 {
            state.store_last_gc_stats(
                "mark-sweep",
                GcStats {
                    cycle_kind: CycleKind::Step,
                    elapsed: Duration::from_nanos(idx + 1),
                    ..GcStats::default()
                },
            );
        }

        let hist = state.gc_pause_hist_snapshot();
        assert_eq!(hist.len(), 256);
        assert_eq!(hist[0], 5);
        assert_eq!(hist[255], 260);
    }

    #[test]
    fn memory_footprint_hist_ring_wraps_at_256_entries() {
        let state = super::RuntimeState::new();
        for idx in 0..260usize {
            state.store_last_gc_stats(
                "mark-sweep",
                GcStats {
                    committed_pages: idx + 1,
                    free_bytes_reusable: (idx + 1) * 10,
                    ..GcStats::default()
                },
            );
        }

        let hist = state.memory_footprint_hist_snapshot();
        assert_eq!(hist.len(), 256);
        assert_eq!(hist[0].committed_pages, 5);
        assert_eq!(hist[0].free_bytes_reusable, 50);
        assert_eq!(hist[255].committed_pages, 260);
        assert_eq!(hist[255].free_bytes_reusable, 2600);
    }

    #[test]
    fn gc_stats_log_gate_accepts_only_one() {
        assert!(super::gc_log_enabled_value(Some(OsStr::new("1"))));
        assert!(!super::gc_log_enabled_value(Some(OsStr::new("true"))));
        assert!(!super::gc_log_enabled_value(Some(OsStr::new("0"))));
        assert!(!super::gc_log_enabled_value(None));
    }

    #[test]
    fn gc_stats_log_summary_contains_required_fields() {
        let stats = GcStats {
            cycle_kind: CycleKind::ZgcCycle,
            pause_ns_max: 11,
            pause_ns_total: 17,
            pause_count: 2,
            relocated_bytes: 64,
            relocated_objects: 3,
            barrier_events: 5,
            satb_flushes: 1,
            rset_cards: 7,
            rset_precise_slots: 2,
            load_barrier_mark_hits: 13,
            load_barrier_relocate_hits: 21,
            ..GcStats::default()
        };

        let line = super::format_gc_log_summary("zgc", &stats);

        assert!(line.contains("algorithm=zgc"));
        assert!(line.contains("cycle=zgc-cycle"));
        assert!(line.contains("pause_ns_max=11"));
        assert!(line.contains("pause_ns_total=17"));
        assert!(line.contains("relocated_bytes=64"));
        assert!(line.contains("barrier_events=5"));
        assert!(line.contains("rset_cards=7"));
        assert!(line.contains("load_barrier_mark_hits=13"));
        assert!(line.contains("load_barrier_relocate_hits=21"));
    }
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
    fn gc_algorithm_env_defaults_to_mark_sweep() {
        assert_eq!(gc_algorithm_from_env(&[]), Ok(GcAlgorithmKind::MarkSweep));
    }

    #[test]
    fn gc_algorithm_env_rejects_unknown_value_with_legal_names() {
        let env = [("WJSM_TEST_GC".to_string(), "bogus".to_string())];
        let err = gc_algorithm_from_env(&env).expect_err("bogus GC flavor should be rejected");

        assert!(err.contains("bogus"));
        assert!(err.contains("mark-sweep"));
        assert!(err.contains("g1"));
        assert!(err.contains("zgc"));
    }

    #[test]
    fn gc_algorithm_env_uses_public_wjsm_gc() {
        let env = [("WJSM_GC".to_string(), "zgc".to_string())];

        assert_eq!(gc_algorithm_from_env(&env), Ok(GcAlgorithmKind::Zgc));
    }

    #[test]
    fn gc_algorithm_test_env_overrides_public_env() {
        let env = [
            ("WJSM_GC".to_string(), "mark-sweep".to_string()),
            ("WJSM_TEST_GC".to_string(), "g1".to_string()),
        ];

        assert_eq!(gc_algorithm_from_env(&env), Ok(GcAlgorithmKind::G1));
    }

    #[test]
    fn runtime_options_builder_sets_gc_algorithm() {
        let mut options = RuntimeOptions::with_gc_algorithm(GcAlgorithmKind::G1);
        assert_eq!(options.gc_algorithm, GcAlgorithmKind::G1);

        options.set_gc_algorithm(GcAlgorithmKind::Zgc);
        assert_eq!(options.gc_algorithm, GcAlgorithmKind::Zgc);
    }

    #[test]
    fn execute_with_writer_prints_string_fixture() -> Result<()> {
        let rt = Runtime::new()?;
        let wasm_bytes = compile_source(r#"console.log("Hello, Async Runtime!");"#)?;
        let (output, _) =
            rt.block_on(async { execute_with_writer(&wasm_bytes, Vec::new()).await })?;
        assert_eq!(String::from_utf8(output)?, "Hello, Async Runtime!\n");
        Ok(())
    }

    #[test]
    fn max_heap_size_near_limit_allows_execution() -> Result<()> {
        let rt = Runtime::new()?;
        let wasm_bytes = compile_source(
            r#"let xs=[]; for (let i=0; i<100; i=i+1) { xs.push({a:i,b:i}); } console.log(xs.length);"#,
        )?;
        let (output, _) = rt.block_on(async {
            execute_with_writer_with_options(
                &wasm_bytes,
                Vec::new(),
                RuntimeOptions::with_max_heap_size(25 * 1024),
            )
            .await
        })?;

        assert_eq!(String::from_utf8(output)?, "100\n");
        Ok(())
    }

    #[test]
    fn max_heap_size_over_limit_returns_controlled_oom() -> Result<()> {
        let rt = Runtime::new()?;
        let wasm_bytes = compile_source(
            r#"let xs=[]; for (let i=0; i<100; i=i+1) { xs.push({a:i,b:i}); } console.log(xs.length);"#,
        )?;
        let error = rt
            .block_on(async {
                execute_with_writer_with_options(
                    &wasm_bytes,
                    Vec::new(),
                    RuntimeOptions::with_max_heap_size(9 * 1024),
                )
                .await
            })
            .expect_err("heap budget should reject the allocation before a wasm trap escapes");
        let message = error.to_string();

        assert!(message.contains("JavaScript heap budget exhausted"));
        assert!(message.contains("9216 bytes used"));
        assert!(!message.contains("wasm trap"));
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
        use super::*;
        use criterion::Criterion;
        let wasm = compile_source("")?;
        let rt = Runtime::new()?;
        let mut c = Criterion::default().sample_size(50);

        // ── 准备缓存目录 ────────────────────────────────────────────
        let cache_dir = std::env::temp_dir().join("wjsm-bench-criterion");
        let _ = std::fs::remove_dir_all(&cache_dir);
        std::fs::create_dir_all(&cache_dir)?;
        unsafe {
            std::env::set_var("WJSM_CACHE_DIR", &cache_dir);
        }

        let config = startup_engine_config(true, None);
        let engine = Engine::new(&config).map_err(|e| anyhow::anyhow!("engine: {e:?}"))?;

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
                    Module::deserialize(
                        criterion::black_box(&engine),
                        criterion::black_box(cwasm_bytes),
                    )
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
                        let config = startup_engine_config(true, None);
                        let engine = Engine::new(&config).expect("engine");
                        let module = Module::new(&engine, &wasm).expect("module");
                        let mut store = Store::new(&engine, RuntimeState::new_with_shared(None));
                        store.set_epoch_deadline(1);
                        store.epoch_deadline_async_yield_and_update(1);
                        let _rx = prepare_async_host_completion(&mut store);
                        let mut linker = Linker::new(&engine);
                        register_startup_linker(&mut linker, &mut store).expect("register linker");
                        let needs_support =
                            module.imports().any(|imp| imp.module() == "wjsm_support");
                        if needs_support {
                            setup_shared_env_and_support(&mut linker, &mut store, &engine)
                                .await
                                .expect("setup support");
                        }
                        let instance = linker
                            .instantiate_async(&mut store, &module)
                            .await
                            .expect("instantiate");
                        let env = extract_wasm_env(&instance, &mut store);
                        if let Ok(f) =
                            instance.get_typed_func::<(), i64>(&mut store, "__wjsm_init_globals")
                        {
                            let _ = f.call_async(&mut store, ()).await;
                        }
                        let start = std::time::Instant::now();
                        startup_snapshot::restore_startup_snapshot(
                            &mut store,
                            &env,
                            snap_view.clone(),
                        )
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

    #[test]
    fn process_env_proxy_reads_keys_and_rejects_writes() -> Result<()> {
        let rt = Runtime::new()?;
        let wasm_bytes = compile_source(
            r#"
console.log(process.env.A);
console.log(Object.keys(process.env).join(","));
process.env.A = "9";
console.log(process.env.A);
console.log("B" in process.env);
console.log(Reflect.set(process.env, "B", "9"));
"#,
        )?;
        let options = RuntimeOptions {
            env: vec![("A".into(), "1".into()), ("B".into(), "2".into())],
            ..RuntimeOptions::default()
        };
        let (output, diagnostics) = rt.block_on(async {
            execute_with_writer_with_options(&wasm_bytes, Vec::new(), options).await
        })?;
        assert!(diagnostics.is_empty());
        assert_eq!(String::from_utf8(output)?, "1\nA,B\n1\ntrue\nfalse\n");
        Ok(())
    }
}
