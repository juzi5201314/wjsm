use super::*;
use crate::runtime_linker::{register_common_bridges, register_complex_bridges, register_linker};

pub(super) fn startup_snapshot_enabled() -> bool {
    // 首次进入 startup 路径时，注入 ABI hash external input：
    // 把 support module layout hash + builtin JS bundle hash 合并为单个 u64，
    // 任一变更都使 embedded snapshot abi_hash 失配 → cold rebuild。
    // OnceLock 重复 set 静默；build.rs 与运行时都安全调用此处。
    wjsm_snapshot_format::register_abi_hash_external_input(combined_abi_external_input());
    // 默认开启；显式设 WJSM_STARTUP_SNAPSHOT=0/off/false 可关闭。
    !matches!(
        std::env::var("WJSM_STARTUP_SNAPSHOT").as_deref(),
        Ok("0") | Ok("false") | Ok("off")
    )
}

/// 把 support module layout hash 与 builtin JS bundle hash 合并为单个稳定 u64。
/// 任一项变化都会使 combined hash 变化 → embedded snapshot ABI mismatch。
pub(super) fn combined_abi_external_input() -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    wjsm_runtime_support::support_abi_union_hash().hash(&mut h);
    builtin_js_bundle_hash().hash(&mut h);
    h.finish()
}

pub(super) fn startup_snapshot_debug_enabled() -> bool {
    matches!(
        std::env::var("WJSM_STARTUP_SNAPSHOT_DEBUG").as_deref(),
        Ok("1") | Ok("true") | Ok("on")
    )
}
/// 解析编译缓存目录。WJSM_CACHE_DIR 优先；未设置时默认 $HOME/.cache/wjsm。
/// 返回 None 表示缓存禁用（WJSM_CACHE_DIR 为空字符串，或 HOME 未设置）。
pub(super) fn module_cache_dir() -> Option<std::path::PathBuf> {
    std::env::var("WJSM_CACHE_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .filter(|h| !h.is_empty())
                .map(|h| std::path::PathBuf::from(h).join(".cache").join("wjsm"))
        })
}

/// 编译或从缓存加载 WASM 模块。
///
/// 缓存 key 是 wasm bytes 的 SipHash，不受二进制 mtime 影响
/// （与 wasmtime 内置 cache 的 debug_assertions mtime keying 不同）。
/// 命中时走 `Module::deserialize`（mmap + 直接加载），跳过 Cranelift 编译。
/// 未命中时 `Module::new` 编译，再 `precompile_module` 持久化到磁盘。
pub(crate) fn compile_or_load_cached(engine: &Engine, wasm_bytes: &[u8]) -> Result<Module> {
    let Some(cache_dir) = module_cache_dir() else {
        return Module::new(engine, wasm_bytes)
            .map_err(|e| anyhow::anyhow!("WASM validation failed: {:?}", e));
    };

    let mut hasher = DefaultHasher::new();
    // wasmtime 版本纳入 hash，避免跨版本缓存冲突
    "wasmtime-43".hash(&mut hasher);
    wasm_bytes.hash(&mut hasher);
    let key = format!("{:016x}", hasher.finish());

    let cache_path = cache_dir.join(&key);

    // 尝试从缓存加载（deserialize_file 走 mmap，零拷贝）
    if cache_path.exists() {
        match unsafe { Module::deserialize_file(engine, &cache_path) } {
            Ok(module) => return Ok(module),
            Err(_) => {
                // 缓存文件损坏或 engine 配置不匹配，删除后重新编译
                let _ = std::fs::remove_file(&cache_path);
            }
        }
    }

    // 编译
    let module = Module::new(engine, wasm_bytes)
        .map_err(|e| anyhow::anyhow!("WASM validation failed: {:?}", e))?;

    // 持久化到缓存（best-effort，失败不影响执行）
    if let Ok(cwasm) = engine.precompile_module(wasm_bytes) {
        let _ = std::fs::create_dir_all(&cache_dir);
        let _ = std::fs::write(&cache_path, &cwasm);
    }

    Ok(module)
}

pub(crate) fn startup_engine_config(
    use_epoch_async_yield: bool,
    wasmtime_memory_reservation: Option<u64>,
    guest_debug: bool,
) -> Config {
    // 冷路径兼容：从环境解析 compiler，构造 Config（不经池）。
    let key = crate::runtime_engine_pool::engine_config_key(
        None,
        use_epoch_async_yield,
        wasmtime_memory_reservation,
        guest_debug,
    );
    crate::runtime_engine_pool::build_engine_config(&key)
}

/// 与 `startup_engine_config` 相同，但接受显式 compiler。
#[allow(dead_code)]
pub(crate) fn startup_engine_config_with_compiler(
    compiler: Option<crate::RuntimeCompiler>,
    use_epoch_async_yield: bool,
    wasmtime_memory_reservation: Option<u64>,
    guest_debug: bool,
) -> Config {
    let key = crate::runtime_engine_pool::engine_config_key(
        compiler,
        use_epoch_async_yield,
        wasmtime_memory_reservation,
        guest_debug,
    );
    crate::runtime_engine_pool::build_engine_config(&key)
}

pub(super) fn register_startup_linker(
    linker: &mut Linker<RuntimeState>,
    store: &mut Store<RuntimeState>,
) -> Result<()> {
    register_linker(linker, store)?;
    register_common_bridges(linker, store)?;
    register_complex_bridges(linker, store)?;
    Ok(())
}

pub(super) fn prepare_async_host_completion(
    store: &mut Store<RuntimeState>,
) -> tokio::sync::mpsc::UnboundedReceiver<crate::scheduler::AsyncHostCompletion> {
    // Phase 6: 创建 channel + counter（仅 async 路径）
    let (host_completion_tx, host_completion_rx) = tokio::sync::mpsc::unbounded_channel();
    store
        .data_mut()
        .host_completion_tx
        .replace(host_completion_tx);
    let counter = crate::scheduler::AsyncOpCounter::new();
    store.data_mut().async_op_counter.replace(counter);
    host_completion_rx
}

pub(super) fn extract_wasm_env(instance: &Instance, store: &mut Store<RuntimeState>) -> WasmEnv {
    let memory = instance
        .get_export(&mut *store, "memory")
        .and_then(|e| e.into_memory())
        .expect("memory");
    let shadow_memory = instance
        .get_export(&mut *store, wjsm_ir::SHADOW_MEMORY_NAME)
        .and_then(|e| e.into_memory())
        .expect("shadow_memory");
    let heap_ptr_global = instance
        .get_export(&mut *store, "__heap_ptr")
        .and_then(|e| e.into_global())
        .expect("heap");
    let obj_table_ptr_global = instance
        .get_export(&mut *store, "__obj_table_ptr")
        .and_then(|e| e.into_global())
        .expect("obj_table_ptr");
    let obj_table_count_global = instance
        .get_export(&mut *store, "__obj_table_count")
        .and_then(|e| e.into_global())
        .expect("obj_table_count");
    let func_table = instance
        .get_export(&mut *store, "__table")
        .and_then(|e| e.into_table())
        .expect("table");
    let shadow_sp_global = instance
        .get_export(&mut *store, "__shadow_sp")
        .and_then(|e| e.into_global())
        .expect("shadow");
    let array_proto_handle_global = instance
        .get_export(&mut *store, "__array_proto_handle")
        .and_then(|e| e.into_global())
        .expect("array_proto");
    let object_proto_handle_global = instance
        .get_export(&mut *store, "__object_proto_handle")
        .and_then(|e| e.into_global())
        .expect("object_proto");

    let wasm_env = wasm_env::WasmEnv {
        memory,
        shadow_memory,
        func_table,
        shadow_sp: shadow_sp_global,
        heap_ptr: heap_ptr_global,
        obj_table_ptr: obj_table_ptr_global,
        obj_table_count: obj_table_count_global,
        shadow_stack_end: instance
            .get_export(&mut *store, "__shadow_stack_end")
            .and_then(|e| e.into_global()),
        object_proto_handle: object_proto_handle_global,
        array_proto_handle: array_proto_handle_global,
        object_heap_start: instance
            .get_export(&mut *store, "__object_heap_start")
            .and_then(|e| e.into_global()),
        bootstrap_done: instance
            .get_export(&mut *store, "__bootstrap_done")
            .and_then(|e| e.into_global()),
        function_props_done: instance
            .get_export(&mut *store, "__function_props_done")
            .and_then(|e| e.into_global()),
        function_props_base: instance
            .get_export(&mut *store, "__function_props_base")
            .and_then(|e| e.into_global()),
        num_ir_functions: instance
            .get_export(&mut *store, "__num_ir_functions")
            .and_then(|e| e.into_global()),
        arr_proto_table_base: instance
            .get_export(&mut *store, "__arr_proto_table_base")
            .and_then(|e| e.into_global()),
        arr_proto_table_len: instance
            .get_export(&mut *store, "__arr_proto_table_len")
            .and_then(|e| e.into_global()),
        arr_proto_table_hash: instance
            .get_export(&mut *store, "__arr_proto_table_hash")
            .and_then(|e| e.into_global()),
        heap_limit: instance
            .get_export(&mut *store, "__heap_limit")
            .and_then(|e| e.into_global()),
        alloc_ptr: instance
            .get_export(&mut *store, "__alloc_ptr")
            .and_then(|e| e.into_global()),
        alloc_end: instance
            .get_export(&mut *store, "__alloc_end")
            .and_then(|e| e.into_global()),
        gc_alloc_bytes: instance
            .get_export(&mut *store, "__gc_alloc_bytes")
            .and_then(|e| e.into_global()),
        gc_trigger_bytes: instance
            .get_export(&mut *store, "__gc_trigger_bytes")
            .and_then(|e| e.into_global()),
        gc_phase: instance
            .get_export(&mut *store, "__gc_phase")
            .and_then(|e| e.into_global()),
        good_color: instance
            .get_export(&mut *store, "__good_color")
            .and_then(|e| e.into_global()),
        barrier_buf_ptr: instance
            .get_export(&mut *store, "__barrier_buf_ptr")
            .and_then(|e| e.into_global()),
        barrier_buf_end: instance
            .get_export(&mut *store, "__barrier_buf_end")
            .and_then(|e| e.into_global()),
    };
    // 缓存供嵌套 host→host 重入：Caller::get_export 在纯 host 调用链上不可用。
    store.data_mut().cached_wasm_env = Some(wasm_env);
    wasm_env
}

fn install_array_iterator_methods(store: &mut Store<RuntimeState>, wasm_env: &WasmEnv) {
    let array_proto_handle = wasm_env
        .array_proto_handle
        .get(&mut *store)
        .i32()
        .unwrap_or(-1);
    if array_proto_handle < 0 {
        return;
    }
    let array_proto = value::encode_object_handle(array_proto_handle as u32);
    // values() 同时作为 [Symbol.iterator]（ES2024 §23.1.3.36：二者是同一函数对象）。
    let values = create_native_callable(store.data(), NativeCallable::ArrayProtoValues);
    let keys = create_native_callable(store.data(), NativeCallable::ArrayProtoKeys);
    let entries = create_native_callable(store.data(), NativeCallable::ArrayProtoEntries);
    let method_flags = constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE;
    for (name, callable) in [("values", values), ("keys", keys), ("entries", entries)] {
        if let Some(name_id) = find_memory_c_string_with_env(store, wasm_env, name)
            .or_else(|| alloc_heap_c_string_with_env(store, wasm_env, name))
        {
            let _ = define_host_data_property_by_name_id_with_env(
                store,
                wasm_env,
                array_proto,
                encode_string_name_id(name_id),
                callable,
                method_flags,
            );
        }
    }
    let _ = define_host_data_property_by_name_id_with_env(
        store,
        wasm_env,
        array_proto,
        encode_symbol_name_id(wjsm_ir::wk_symbol::ITERATOR),
        values,
        method_flags,
    );
}

fn startup_runtime_error(store: &Store<RuntimeState>) -> Option<String> {
    store
        .data()
        .runtime_error
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn ensure_startup_object(store: &Store<RuntimeState>, value: i64, label: &str) -> Result<()> {
    if value::is_object(value) {
        return Ok(());
    }
    if let Some(message) = startup_runtime_error(store) {
        anyhow::bail!(message);
    }
    anyhow::bail!("startup allocation failed for {label}")
}

fn ensure_no_startup_error(store: &Store<RuntimeState>) -> Result<()> {
    if let Some(message) = startup_runtime_error(store) {
        anyhow::bail!(message);
    }
    Ok(())
}

pub(super) fn initialize_host_post_bootstrap(
    store: &mut Store<RuntimeState>,
    wasm_env: &WasmEnv,
) -> Result<()> {
    if wasm_env.obj_table_count.get(&mut *store).i32().unwrap_or(0) == 0 {
        // handle 0 仍作为旧原型链 null 哨兵；host primordial 从 1 开始，避免 Object.getPrototypeOf 误判。
        let null_sentinel = alloc_host_object(store, wasm_env, 0);
        ensure_startup_object(store, null_sentinel, "null sentinel")?;
    }
    install_array_iterator_methods(store, wasm_env);
    ensure_no_startup_error(store)?;

    let iterator_proto = alloc_host_object(store, wasm_env, 2);
    ensure_startup_object(store, iterator_proto, "IteratorPrototype")?;
    let iterator_symbol_iterator = create_iterator_proto_identity(store.data());
    let _ = define_host_data_property_by_name_id_with_env(
        store,
        wasm_env,
        iterator_proto,
        encode_symbol_name_id(wjsm_ir::wk_symbol::ITERATOR),
        iterator_symbol_iterator,
        constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
    );
    let iterator_tag = store_runtime_string_in_state(store.data(), "Iterator".to_string());
    let _ = define_host_data_property_with_env(
        store,
        wasm_env,
        iterator_proto,
        "Symbol.toStringTag",
        iterator_tag,
    );
    ensure_no_startup_error(store)?;

    let generator_proto = alloc_host_object(store, wasm_env, 2);
    ensure_startup_object(store, generator_proto, "GeneratorPrototype")?;
    let generator_handle = value::decode_object_handle(generator_proto);
    let iterator_handle = value::decode_object_handle(iterator_proto);
    let Some(generator_ptr) =
        resolve_handle_idx_with_env(store, wasm_env, generator_handle as usize)
    else {
        anyhow::bail!("startup allocation failed for GeneratorPrototype");
    };
    let data = wasm_env.memory.data_mut(&mut *store);
    data[generator_ptr..generator_ptr + 4].copy_from_slice(&iterator_handle.to_le_bytes());
    let generator_tag = store_runtime_string_in_state(store.data(), "Generator".to_string());
    let _ = define_host_data_property_with_env(
        store,
        wasm_env,
        generator_proto,
        "Symbol.toStringTag",
        generator_tag,
    );
    ensure_no_startup_error(store)?;

    let async_iterator_proto = alloc_host_object(store, wasm_env, 2);
    ensure_startup_object(store, async_iterator_proto, "AsyncIteratorPrototype")?;
    let async_iterator_symbol_async_iterator = create_native_callable(
        store.data(),
        NativeCallable::AsyncIteratorProtoSymbolAsyncIterator,
    );
    let _ = define_host_data_property_by_name_id_with_env(
        store,
        wasm_env,
        async_iterator_proto,
        encode_symbol_name_id(wjsm_ir::wk_symbol::ASYNC_ITERATOR),
        async_iterator_symbol_async_iterator,
        constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
    );
    let async_iterator_tag =
        store_runtime_string_in_state(store.data(), "AsyncIterator".to_string());
    let _ = define_host_data_property_with_env(
        store,
        wasm_env,
        async_iterator_proto,
        "Symbol.toStringTag",
        async_iterator_tag,
    );
    ensure_no_startup_error(store)?;

    let async_gen_proto = alloc_host_object(store, wasm_env, 2);
    ensure_startup_object(store, async_gen_proto, "AsyncGeneratorPrototype")?;
    let async_gen_handle = value::decode_object_handle(async_gen_proto);
    let async_iterator_handle = value::decode_object_handle(async_iterator_proto);
    let Some(async_gen_ptr) =
        resolve_handle_idx_with_env(store, wasm_env, async_gen_handle as usize)
    else {
        anyhow::bail!("startup allocation failed for AsyncGeneratorPrototype");
    };
    let data = wasm_env.memory.data_mut(&mut *store);
    data[async_gen_ptr..async_gen_ptr + 4].copy_from_slice(&async_iterator_handle.to_le_bytes());
    let async_gen_tag = store_runtime_string_in_state(store.data(), "AsyncGenerator".to_string());
    let _ = define_host_data_property_with_env(
        store,
        wasm_env,
        async_gen_proto,
        "Symbol.toStringTag",
        async_gen_tag,
    );
    ensure_no_startup_error(store)?;

    store.data_mut().iterator_prototype = iterator_proto;
    store.data_mut().generator_prototype = generator_proto;
    store.data_mut().async_iterator_prototype = async_iterator_proto;
    store.data_mut().async_gen_prototype = async_gen_proto;
    Ok(())
}

pub(crate) struct ExecuteInstanceBundle {
    pub(crate) store: Store<RuntimeState>,
    pub(crate) instance: Instance,
    pub(crate) wasm_env: WasmEnv,
    pub(super) output: Arc<Mutex<Vec<u8>>>,
    pub(super) runtime_error: Arc<Mutex<Option<String>>>,
    pub(super) diagnostics: Arc<Mutex<Vec<u8>>>,
    pub(super) host_completion_rx:
        tokio::sync::mpsc::UnboundedReceiver<crate::scheduler::AsyncHostCompletion>,
}

pub(crate) async fn instantiate_execute_bundle(
    engine: &Engine,
    module: &Module,
    shared_state: Option<Arc<SharedRuntimeState>>,
    use_epoch_async_yield: bool,
    options: RuntimeOptions,
) -> Result<ExecuteInstanceBundle> {
    instantiate_execute_bundle_with_epoch(
        engine,
        module,
        shared_state,
        use_epoch_async_yield,
        options,
        None,
    )
    .await
}

pub(crate) async fn instantiate_execute_bundle_with_epoch(
    engine: &Engine,
    module: &Module,
    shared_state: Option<Arc<SharedRuntimeState>>,
    use_epoch_async_yield: bool,
    options: RuntimeOptions,
    epoch: Option<Arc<crate::runtime_engine_pool::EpochController>>,
) -> Result<ExecuteInstanceBundle> {
    let mut state = RuntimeState::new_with_shared_and_options(shared_state, options)?;
    state.epoch_controller = epoch;
    let mut store = Store::new(engine, state);
    let output = Arc::clone(&store.data().output);
    let runtime_error = Arc::clone(&store.data().runtime_error);
    let diagnostics = Arc::clone(&store.data().diagnostics);
    if use_epoch_async_yield {
        crate::runtime_engine_pool::install_epoch_deadline_callback(&mut store);
    }
    let host_completion_rx = prepare_async_host_completion(&mut store);
    // worker_threads：注册 parentPort wake，并默认 ref 住 parentPort（与 Node 一致，
    // 防止 main 结束后 agent 立刻退出，需 terminate/close 才放行）。
    if store.data().is_worker_thread
        && let Some(port_id) = store.data().parent_port_id
    {
        crate::runtime_node_worker_threads::register_worker_port_wake(&mut store, port_id, None);
        crate::runtime_node_worker_threads::auto_ref_port_on_store(&mut store, port_id);
    }
    let mut linker = Linker::new(engine);
    register_startup_linker(&mut linker, &mut store)?;

    // P2.2+P2.3: 如果 user module import "wjsm_support" namespace，需要先 instantiate
    // support module 并把它的 exports 注册到 linker 的 "wjsm_support" namespace。
    // 同时创建 shared memory/table/globals 注册到 "env" namespace。
    let needs_support = module
        .imports()
        .any(|import| import.module() == "wjsm_support");
    if needs_support {
        setup_shared_env_and_support(&mut linker, &mut store, engine).await?;
    }

    let instance = linker
        .instantiate_async(&mut store, module)
        .await
        .map_err(|e| anyhow::anyhow!("async instantiate failed: {:?}", e))?;
    let wasm_env = extract_wasm_env(&instance, &mut store);

    Ok(ExecuteInstanceBundle {
        store,
        instance,
        wasm_env,
        output,
        runtime_error,
        diagnostics,
        host_completion_rx,
    })
}

fn heap_limit_base(store: &mut Store<RuntimeState>, wasm_env: &WasmEnv) -> usize {
    let (_, dynamic_heap_start, _) = store.data().heap_layout_boundaries();
    if dynamic_heap_start != 0 {
        return dynamic_heap_start;
    }
    wasm_env
        .object_heap_start
        .and_then(|g| g.get(&mut *store).i32())
        .unwrap_or(0)
        .max(0) as usize
}

fn configured_heap_limit(store: &mut Store<RuntimeState>, wasm_env: &WasmEnv) -> Result<u32> {
    let Some(max_heap_size) = store.data().max_heap_size() else {
        return Ok(u32::MAX);
    };
    let heap_start = heap_limit_base(store, wasm_env);
    let limit = heap_start.checked_add(max_heap_size).ok_or_else(|| {
        anyhow::anyhow!("max heap size exceeds wasm32 heap address space: {max_heap_size} bytes")
    })?;
    if limit > u32::MAX as usize {
        anyhow::bail!("max heap size exceeds wasm32 heap address space: {max_heap_size} bytes");
    }
    Ok(limit as u32)
}

fn enforce_heap_limit(store: &mut Store<RuntimeState>, wasm_env: &WasmEnv) -> Result<()> {
    let Some(heap_limit) = wasm_env.heap_limit else {
        if store.data().max_heap_size().is_some() {
            anyhow::bail!("module does not expose __heap_limit for max heap enforcement");
        }
        return Ok(());
    };
    let limit = configured_heap_limit(store, wasm_env)?;
    heap_limit.set(&mut *store, Val::I32(limit as i32))?;
    let heap_ptr = wasm_env.heap_ptr.get(&mut *store).i32().unwrap_or(0).max(0) as usize;
    if heap_ptr > limit as usize {
        let heap_start = heap_limit_base(store, wasm_env);
        let used = heap_ptr.saturating_sub(heap_start);
        store.data().set_heap_oom_error(used, 0);
        anyhow::bail!(store.data().heap_oom_message(used, 0));
    }
    Ok(())
}

fn align_gc_region_start(value: usize) -> Result<usize> {
    let align = wjsm_ir::constants::GC_REGION_SIZE as usize;
    value
        .checked_add(align - 1)
        .map(|v| v & !(align - 1))
        .ok_or_else(|| anyhow::anyhow!("GC dynamic heap start exceeds address space"))
}

fn record_and_attach_gc_heap(
    store: &mut Store<RuntimeState>,
    wasm_env: &WasmEnv,
    immortal_objects_end: usize,
) -> Result<()> {
    let object_heap_start = wasm_env
        .object_heap_start
        .and_then(|g| g.get(&mut *store).i32())
        .unwrap_or(0)
        .max(0) as usize;
    let immortal_objects_end = immortal_objects_end.max(object_heap_start);
    let dynamic_heap_start = align_gc_region_start(immortal_objects_end)?;
    if dynamic_heap_start > i32::MAX as usize {
        anyhow::bail!("GC dynamic heap start exceeds wasm32 signed global range");
    }
    wasm_env
        .heap_ptr
        .set(&mut *store, Val::I32(dynamic_heap_start as i32))?;
    // barrier 位于主内存 handle table 之后（不再嵌在 shadow 区后）。
    let barrier_event_buf_base = wasm_env
        .barrier_buf_ptr
        .and_then(|g| g.get(&mut *store).i32())
        .unwrap_or(0)
        .max(0) as usize;
    let barrier_event_buf_end = wasm_env
        .barrier_buf_end
        .and_then(|g| g.get(&mut *store).i32())
        .map(|v| v.max(0) as usize)
        .unwrap_or_else(|| {
            barrier_event_buf_base + wjsm_ir::constants::GC_BARRIER_EVENT_BUFFER_SIZE as usize
        });
    if barrier_event_buf_end > i32::MAX as usize {
        anyhow::bail!("GC barrier buffer end exceeds wasm32 signed global range");
    }
    if let Some(global) = wasm_env.gc_alloc_bytes {
        global.set(&mut *store, Val::I32(0))?;
    }
    if let Some(global) = wasm_env.gc_phase {
        global.set(&mut *store, Val::I32(0))?;
    }
    if let Some(global) = wasm_env.good_color {
        global.set(&mut *store, Val::I32(0))?;
    }
    if let Some(global) = wasm_env.barrier_buf_ptr {
        global.set(&mut *store, Val::I32(barrier_event_buf_base as i32))?;
    }
    if let Some(global) = wasm_env.barrier_buf_end {
        global.set(&mut *store, Val::I32(barrier_event_buf_end as i32))?;
    }

    store.data().store_heap_layout_boundaries(
        immortal_objects_end,
        dynamic_heap_start,
        barrier_event_buf_base,
    );
    enforce_heap_limit(store, wasm_env)?;
    let (_, attached_dynamic_heap_start, _) = store.data().heap_layout_boundaries();
    if let Some(global) = wasm_env.gc_trigger_bytes {
        global.set(
            &mut *store,
            Val::I32(wjsm_ir::constants::GC_INITIAL_TRIGGER_BYTES as i32),
        )?;
    }

    let gc_arc = store.data().gc_algorithm.clone();
    let mut gc = gc_arc.lock().unwrap_or_else(|e| e.into_inner());
    let mut gc_ctx = crate::runtime_gc::GcContext::new(store, wasm_env, gc.name());
    gc.attach_heap(&mut gc_ctx, attached_dynamic_heap_start);
    Ok(())
}

/// P2.2+P2.3: 创建 shared memory/table/globals 注册到 "env" namespace，
/// 然后 instantiate support module 并把 exports 注册到 "wjsm_support" namespace。
pub(super) async fn setup_shared_env_and_support(
    linker: &mut Linker<RuntimeState>,
    store: &mut Store<RuntimeState>,
    engine: &Engine,
) -> Result<()> {
    // 创建 shared main memory (8 pages)
    let memory = Memory::new(&mut *store, MemoryType::new(8, None))?;
    linker.define(&*store, "env", "memory", memory)?;

    // 独立影子栈 memory：冷启动 1 页，软上限来自 RuntimeOptions。
    let soft_max = store.data().shadow_stack_max();
    let initial_pages = (wjsm_ir::SHADOW_STACK_INITIAL_SIZE as u64)
        .div_ceil(65536)
        .max(1) as u32;
    let max_pages = ((soft_max as u64).div_ceil(65536).max(initial_pages as u64)) as u32;
    let shadow_memory = Memory::new(
        &mut *store,
        MemoryType::new(initial_pages, Some(max_pages.max(initial_pages))),
    )?;
    linker.define(&*store, "env", wjsm_ir::SHADOW_MEMORY_NAME, shadow_memory)?;

    // 创建 shared funcref table。
    // 间接调用 / 闭包 / 方法表会写入 element section；cluster+net 等大型 builtin
    // 组合可超过 256（实测 ~261+）。min 过小会在 instantiate 时
    // trap: undefined element: out of bounds table access。
    // 上限不封顶（None），初始给足常用组合 + support 预留余量。
    const SHARED_FUNCREF_TABLE_MIN: u32 = 2048;
    let table = Table::new(
        &mut *store,
        TableType::new(RefType::FUNCREF, SHARED_FUNCREF_TABLE_MIN, None),
        Ref::Func(None),
    )?;
    linker.define(&*store, "env", "__table", table)?;

    // 创建 27 个 shared globals（全部 mutable，user bootstrap 中用 global.set 初始化）
    // 顺序与 abi::ENV_GLOBALS 和 compiler_core.rs::ENV_GLOBAL_EXPORT_NAMES 对齐。
    for (name, ty, init) in [
        ("__func_props", ValType::I32, Val::I32(0)),
        ("__heap_ptr", ValType::I32, Val::I32(0)),
        ("__obj_table_ptr", ValType::I32, Val::I32(0)),
        ("__obj_table_count", ValType::I32, Val::I32(0)),
        ("__shadow_sp", ValType::I32, Val::I32(0)),
        ("__object_heap_start", ValType::I32, Val::I32(0)),
        ("__num_ir_functions", ValType::I32, Val::I32(0)),
        ("__shadow_stack_end", ValType::I32, Val::I32(0)),
        ("__array_proto_handle", ValType::I32, Val::I32(-1)),
        ("__object_proto_handle", ValType::I32, Val::I32(-1)),
        ("__eval_var_map_ptr", ValType::I32, Val::I32(0)),
        ("__eval_var_map_count", ValType::I32, Val::I32(0)),
        ("__bootstrap_done", ValType::I32, Val::I32(0)),
        ("__function_props_done", ValType::I32, Val::I32(0)),
        ("__function_props_base", ValType::I32, Val::I32(0)),
        ("__arr_proto_table_base", ValType::I32, Val::I32(0)),
        ("__arr_proto_table_len", ValType::I32, Val::I32(0)),
        ("__arr_proto_table_hash", ValType::I64, Val::I64(0)),
        ("__heap_limit", ValType::I32, Val::I32(-1)),
        ("__alloc_ptr", ValType::I32, Val::I32(0)),
        ("__alloc_end", ValType::I32, Val::I32(0)),
        ("__gc_alloc_bytes", ValType::I32, Val::I32(0)),
        (
            "__gc_trigger_bytes",
            ValType::I32,
            Val::I32(wjsm_ir::constants::GC_INITIAL_TRIGGER_BYTES as i32),
        ),
        ("__gc_phase", ValType::I32, Val::I32(0)),
        ("__good_color", ValType::I32, Val::I32(0)),
        ("__barrier_buf_ptr", ValType::I32, Val::I32(0)),
        ("__barrier_buf_end", ValType::I32, Val::I32(0)),
    ] {
        define_env_global(linker, store, name, ty, true, init);
    }

    let gc_kind = {
        let gc = store
            .data()
            .gc_algorithm
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match gc.name() {
            "g1" => crate::GcAlgorithmKind::G1,
            "zgc" => crate::GcAlgorithmKind::Zgc,
            _ => crate::GcAlgorithmKind::MarkSweep,
        }
    };
    let support_flavor = match gc_kind {
        crate::GcAlgorithmKind::MarkSweep => wjsm_backend_wasm::GcFlavor::MarkSweep,
        crate::GcAlgorithmKind::G1 => wjsm_backend_wasm::GcFlavor::G1,
        crate::GcAlgorithmKind::Zgc => wjsm_backend_wasm::GcFlavor::Zgc,
    };

    // 获取 support module：优先从 embedded cwasm deserialize，否则从 emit_support_module 编译。
    // cwasm 的 precompile 配置必须与运行时 engine 配置匹配（epoch interruption 等），
    // 不匹配时 fallback 到 Module::new 从 wasm bytes 编译。
    let support_module = if let Some(cwasm_bytes) = embedded_support_cwasm_for(gc_kind) {
        match unsafe { Module::deserialize(engine, cwasm_bytes) } {
            Ok(m) => m,
            Err(_) => {
                // cwasm 配置不匹配（如 epoch interruption），fallback 到从 wasm bytes 编译
                let support_wasm = wjsm_backend_wasm::emit_support_module(support_flavor)?;
                Module::new(engine, &support_wasm)
                    .map_err(|e| anyhow::anyhow!("support module compile failed: {:?}", e))?
            }
        }
    } else {
        // build-time snapshot 生成路径：没有 embedded cwasm，直接从 emit_support_module 编译。
        let support_wasm = wjsm_backend_wasm::emit_support_module(support_flavor)?;
        Module::new(engine, &support_wasm)
            .map_err(|e| anyhow::anyhow!("support module compile failed: {:?}", e))?
    };

    // instantiate support module
    let support_instance = linker
        .instantiate_async(&mut *store, &support_module)
        .await
        .map_err(|e| anyhow::anyhow!("support module instantiate failed: {:?}", e))?;

    let mut support_exports = Vec::with_capacity(wjsm_runtime_support::abi::SUPPORT_EXPORTS.len());
    for export_name in wjsm_runtime_support::abi::SUPPORT_EXPORTS {
        let export = support_instance
            .get_export(&mut *store, export_name)
            .ok_or_else(|| anyhow::anyhow!("support module missing export: {}", export_name))?;
        linker.define(&*store, "wjsm_support", export_name, export.clone())?;
        support_exports.push((*export_name, export));
    }
    *store
        .data()
        .support_exports
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = support_exports;

    Ok(())
}

pub(super) fn define_env_global(
    linker: &mut Linker<RuntimeState>,
    store: &mut Store<RuntimeState>,
    name: &str,
    val_type: ValType,
    mutable: bool,
    init: Val,
) {
    let gty = GlobalType::new(
        val_type,
        if mutable {
            Mutability::Var
        } else {
            Mutability::Const
        },
    );
    let g = Global::new(&mut *store, gty, init).expect("create env global");
    linker
        .define(&*store, "env", name, g)
        .expect("define env global");
}

/// 仅执行 bootstrap 逻辑（host post-bootstrap + __wjsm_bootstrap_once），不触发 snapshot capture。
/// 供 build-time snapshot 生成使用，避免构建期执行用户 main。
pub(super) async fn run_bootstrap_only(bundle: &mut ExecuteInstanceBundle) -> Result<()> {
    run_init_globals_only(bundle).await?;
    initialize_host_post_bootstrap(&mut bundle.store, &bundle.wasm_env)?;
    if let Ok(bootstrap_fn) = bundle
        .instance
        .get_typed_func::<(), i64>(&mut bundle.store, "__wjsm_bootstrap_once")
    {
        if let Err(error) = bootstrap_fn.call_async(&mut bundle.store, ()).await {
            if let Some(message) = startup_runtime_error(&bundle.store) {
                anyhow::bail!(message);
            }
            anyhow::bail!("bootstrap_once failed: {error:?}");
        }
        ensure_no_startup_error(&bundle.store)?;
    }
    run_current_module_function_props(bundle).await?;
    install_array_iterator_methods(&mut bundle.store, &bundle.wasm_env);
    crate::runtime_heap::ensure_error_prototypes_initialized(&mut bundle.store, &bundle.wasm_env);
    crate::runtime_heap::ensure_symbol_prototype_initialized(&mut bundle.store, &bundle.wasm_env);
    crate::runtime_heap::ensure_promise_prototype_initialized(&mut bundle.store, &bundle.wasm_env);
    crate::runtime_heap::ensure_function_prototype_initialized(&mut bundle.store, &bundle.wasm_env);
    crate::runtime_heap::install_function_props_prototypes(&mut bundle.store, &bundle.wasm_env);
    crate::runtime_heap::ensure_regexp_prototype_initialized(&mut bundle.store, &bundle.wasm_env);
    ensure_no_startup_error(&bundle.store)?;
    crate::runtime_node_perf_hooks::mark_bootstrap_complete(bundle.store.data());
    Ok(())
}

async fn run_current_module_function_props(bundle: &mut ExecuteInstanceBundle) -> Result<()> {
    if let Ok(function_props_fn) = bundle
        .instance
        .get_typed_func::<(), i64>(&mut bundle.store, "__wjsm_init_function_props")
    {
        if let Err(error) = function_props_fn.call_async(&mut bundle.store, ()).await {
            if let Some(message) = startup_runtime_error(&bundle.store) {
                anyhow::bail!(message);
            }
            anyhow::bail!("init_function_props failed: {error:?}");
        }
        ensure_no_startup_error(&bundle.store)?;
    }
    Ok(())
}

/// 只设置 imported globals（`__wjsm_init_globals`）。Snapshot restore 前调用；不分配 bootstrap 堆对象。
/// 泄漏的 cold-bootstrap 状态由 `reset_primordial_heap_before_restore` 清除。
pub(super) async fn run_init_globals_only(bundle: &mut ExecuteInstanceBundle) -> Result<()> {
    if let Ok(init_globals_fn) = bundle
        .instance
        .get_typed_func::<(), i64>(&mut bundle.store, "__wjsm_init_globals")
    {
        init_globals_fn
            .call_async(&mut bundle.store, ())
            .await
            .map_err(|e| anyhow::anyhow!("init_globals failed: {e:?}"))?;
    }
    enforce_heap_limit(&mut bundle.store, &bundle.wasm_env)?;
    Ok(())
}

/// 执行 cold startup：跑 bootstrap 后划定 immortal/dynamic 边界，不在客户机器上 capture/write snapshot。
pub(crate) async fn run_startup_cold_path(bundle: &mut ExecuteInstanceBundle) -> Result<()> {
    run_bootstrap_only(bundle).await?;
    let immortal_objects_end = bundle
        .wasm_env
        .heap_ptr
        .get(&mut bundle.store)
        .i32()
        .unwrap_or(0)
        .max(0) as usize;
    record_and_attach_gc_heap(&mut bundle.store, &bundle.wasm_env, immortal_objects_end)?;
    Ok(())
}

pub(super) async fn try_restore_snapshot(
    bundle: &mut ExecuteInstanceBundle,
    snap_bytes: &[u8],
) -> bool {
    let view = match startup_snapshot_format::decode_snapshot(snap_bytes) {
        Ok(v) => v,
        Err(e) => {
            if startup_snapshot_debug_enabled() {
                eprintln!("startup snapshot decode failed: {e:#}");
            }
            return false;
        }
    };
    if let Err(e) =
        startup_snapshot::restore_startup_snapshot(&mut bundle.store, &bundle.wasm_env, view)
    {
        if startup_snapshot_debug_enabled() {
            eprintln!("startup snapshot restore failed: {e:#}");
        }
        return false;
    }
    if let Err(e) = enforce_heap_limit(&mut bundle.store, &bundle.wasm_env) {
        if startup_snapshot_debug_enabled() {
            eprintln!("startup snapshot restore failed: {e:#}");
        }
        return false;
    }
    if let Err(e) = run_current_module_function_props(bundle).await {
        if startup_snapshot_debug_enabled() {
            eprintln!("startup snapshot restore failed: {e:#}");
        }
        return false;
    }
    crate::runtime_heap::ensure_function_prototype_initialized(&mut bundle.store, &bundle.wasm_env);
    crate::runtime_heap::install_function_props_prototypes(&mut bundle.store, &bundle.wasm_env);
    let immortal_objects_end = bundle
        .wasm_env
        .heap_ptr
        .get(&mut bundle.store)
        .i32()
        .unwrap_or(0)
        .max(0) as usize;
    if let Err(e) =
        record_and_attach_gc_heap(&mut bundle.store, &bundle.wasm_env, immortal_objects_end)
    {
        if startup_snapshot_debug_enabled() {
            eprintln!("startup snapshot restore failed: {e:#}");
        }
        return false;
    }
    crate::runtime_node_perf_hooks::mark_bootstrap_complete(bundle.store.data());
    true
}
