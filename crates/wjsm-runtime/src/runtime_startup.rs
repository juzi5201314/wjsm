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
    wjsm_runtime_support::support_module_layout_hash().hash(&mut h);
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
pub(super) fn compile_or_load_cached(engine: &Engine, wasm_bytes: &[u8]) -> Result<Module> {
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

pub(super) fn startup_engine_config(use_epoch_async_yield: bool) -> Config {
    let mut config = Config::new();
    // WJSM_COMPILER=winch 使用 Winch 基线编译器
    if std::env::var("WJSM_COMPILER").as_deref() == Ok("winch") {
        config.strategy(Strategy::Winch);
    }
    // WJSM_OPT_LEVEL=none|speed_and_size 控制 Cranelift 优化等级
    match std::env::var("WJSM_OPT_LEVEL").as_deref() {
        Ok("none") => {
            config.cranelift_opt_level(OptLevel::None);
        }
        Ok("speed_and_size") => {
            config.cranelift_opt_level(OptLevel::SpeedAndSize);
        }
        _ => {}
    }
    if use_epoch_async_yield {
        config.epoch_interruption(true);
    }
    config.wasm_bulk_memory(true);
    config
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

    wasm_env::WasmEnv {
        memory,
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
    }
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
    let array_iterator = create_native_callable(store.data(), NativeCallable::ArrayProtoValues);
    let array_proto = value::encode_object_handle(array_proto_handle as u32);
    let _ = define_host_data_property_by_name_id_with_env(
        store,
        wasm_env,
        array_proto,
        encode_symbol_name_id(wjsm_ir::wk_symbol::ITERATOR),
        array_iterator,
        constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
    );
}

pub(super) fn initialize_host_post_bootstrap(store: &mut Store<RuntimeState>, wasm_env: &WasmEnv) {
    if wasm_env.obj_table_count.get(&mut *store).i32().unwrap_or(0) == 0 {
        // handle 0 仍作为旧原型链 null 哨兵；host primordial 从 1 开始，避免 Object.getPrototypeOf 误判。
        let _ = alloc_host_object(store, wasm_env, 0);
    }
    install_array_iterator_methods(store, wasm_env);
    let async_iterator_proto = alloc_host_object(store, wasm_env, 2);
    let async_iterator_symbol_async_iterator = {
        let mut table = store
            .data()
            .native_callables
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(NativeCallable::AsyncIteratorProtoSymbolAsyncIterator);
        value::encode_native_callable_idx(handle)
    };
    let _ = define_host_data_property_by_name_id_with_env(
        store,
        wasm_env,
        async_iterator_proto,
        encode_symbol_name_id(3),
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
    let async_gen_proto = alloc_host_object(store, wasm_env, 2);
    let async_gen_handle = value::decode_object_handle(async_gen_proto);
    let async_iterator_handle = value::decode_object_handle(async_iterator_proto);
    let obj_ptr =
        resolve_handle_idx_with_env(store, wasm_env, async_gen_handle as usize).expect("obj_ptr");
    let data = wasm_env.memory.data_mut(&mut *store);
    data[obj_ptr..obj_ptr + 4].copy_from_slice(&async_iterator_handle.to_le_bytes());
    let async_gen_tag = store_runtime_string_in_state(store.data(), "AsyncGenerator".to_string());
    let _ = define_host_data_property_with_env(
        store,
        wasm_env,
        async_gen_proto,
        "Symbol.toStringTag",
        async_gen_tag,
    );
    store.data_mut().async_iterator_prototype = async_iterator_proto;
    store.data_mut().async_gen_prototype = async_gen_proto;
}

pub(super) struct ExecuteInstanceBundle {
    pub(super) store: Store<RuntimeState>,
    pub(super) instance: Instance,
    pub(super) wasm_env: WasmEnv,
    pub(super) output: Arc<Mutex<Vec<u8>>>,
    pub(super) runtime_error: Arc<Mutex<Option<String>>>,
    pub(super) diagnostics: Arc<Mutex<Vec<u8>>>,
    pub(super) host_completion_rx:
        tokio::sync::mpsc::UnboundedReceiver<crate::scheduler::AsyncHostCompletion>,
}

pub(super) async fn instantiate_execute_bundle(
    engine: &Engine,
    module: &Module,
    shared_state: Option<Arc<SharedRuntimeState>>,
    use_epoch_async_yield: bool,
) -> Result<ExecuteInstanceBundle> {
    let mut store = Store::new(engine, RuntimeState::new_with_shared(shared_state));
    let output = Arc::clone(&store.data().output);
    let runtime_error = Arc::clone(&store.data().runtime_error);
    let diagnostics = Arc::clone(&store.data().diagnostics);
    if use_epoch_async_yield {
        store.set_epoch_deadline(1);
        store.epoch_deadline_async_yield_and_update(1);
    }
    let host_completion_rx = prepare_async_host_completion(&mut store);
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

/// P2.2+P2.3: 创建 shared memory/table/globals 注册到 "env" namespace，
/// 然后 instantiate support module 并把 exports 注册到 "wjsm_support" namespace。
pub(super) async fn setup_shared_env_and_support(
    linker: &mut Linker<RuntimeState>,
    store: &mut Store<RuntimeState>,
    engine: &Engine,
) -> Result<()> {
    // 创建 shared memory (4 pages = 256KB)
    let memory = Memory::new(&mut *store, MemoryType::new(4, None))?;
    linker.define(&*store, "env", "memory", memory)?;

    // 创建 shared table (minimum 256, 覆盖 support 12 + user ~200 函数)
    let table = Table::new(
        &mut *store,
        TableType::new(RefType::FUNCREF, 256, None),
        Ref::Func(None),
    )?;
    linker.define(&*store, "env", "__table", table)?;

    // 创建 19 个 shared globals（全部 mutable，user bootstrap 中用 global.set 初始化）
    // 顺序与 abi::ENV_GLOBALS 和 compiler_core.rs::ENV_GLOBAL_EXPORT_NAMES 对齐。
    define_env_global(
        linker,
        store,
        "__func_props",
        ValType::I32,
        true,
        Val::I32(0),
    );
    define_env_global(linker, store, "__heap_ptr", ValType::I32, true, Val::I32(0));
    define_env_global(
        linker,
        store,
        "__obj_table_ptr",
        ValType::I32,
        true,
        Val::I32(0),
    );
    define_env_global(
        linker,
        store,
        "__obj_table_count",
        ValType::I32,
        true,
        Val::I32(0),
    );
    define_env_global(
        linker,
        store,
        "__shadow_sp",
        ValType::I32,
        true,
        Val::I32(0),
    );
    define_env_global(
        linker,
        store,
        "__alloc_counter",
        ValType::I32,
        true,
        Val::I32(0),
    );
    define_env_global(
        linker,
        store,
        "__object_heap_start",
        ValType::I32,
        true,
        Val::I32(0),
    );
    define_env_global(
        linker,
        store,
        "__num_ir_functions",
        ValType::I32,
        true,
        Val::I32(0),
    );
    define_env_global(
        linker,
        store,
        "__shadow_stack_end",
        ValType::I32,
        true,
        Val::I32(0),
    );
    define_env_global(
        linker,
        store,
        "__array_proto_handle",
        ValType::I32,
        true,
        Val::I32(-1),
    );
    define_env_global(
        linker,
        store,
        "__object_proto_handle",
        ValType::I32,
        true,
        Val::I32(-1),
    );
    define_env_global(
        linker,
        store,
        "__eval_var_map_ptr",
        ValType::I32,
        true,
        Val::I32(0),
    );
    define_env_global(
        linker,
        store,
        "__eval_var_map_count",
        ValType::I32,
        true,
        Val::I32(0),
    );
    define_env_global(
        linker,
        store,
        "__bootstrap_done",
        ValType::I32,
        true,
        Val::I32(0),
    );
    define_env_global(
        linker,
        store,
        "__function_props_done",
        ValType::I32,
        true,
        Val::I32(0),
    );
    define_env_global(
        linker,
        store,
        "__function_props_base",
        ValType::I32,
        true,
        Val::I32(0),
    );
    define_env_global(
        linker,
        store,
        "__arr_proto_table_base",
        ValType::I32,
        true,
        Val::I32(0),
    );
    define_env_global(
        linker,
        store,
        "__arr_proto_table_len",
        ValType::I32,
        true,
        Val::I32(0),
    );
    define_env_global(
        linker,
        store,
        "__arr_proto_table_hash",
        ValType::I64,
        true,
        Val::I64(0),
    );

    // 获取 support module：优先从 embedded cwasm deserialize，否则从 emit_support_module 编译。
    // cwasm 的 precompile 配置必须与运行时 engine 配置匹配（epoch interruption 等），
    // 不匹配时 fallback 到 Module::new 从 wasm bytes 编译。
    let support_module = if let Some(cwasm_bytes) = embedded_support_cwasm() {
        match unsafe { Module::deserialize(engine, cwasm_bytes) } {
            Ok(m) => m,
            Err(_) => {
                // cwasm 配置不匹配（如 epoch interruption），fallback 到从 wasm bytes 编译
                let support_wasm = wjsm_backend_wasm::emit_support_module()?;
                Module::new(engine, &support_wasm)
                    .map_err(|e| anyhow::anyhow!("support module compile failed: {:?}", e))?
            }
        }
    } else {
        // build-time snapshot 生成路径：没有 embedded cwasm，直接从 emit_support_module 编译。
        let support_wasm = wjsm_backend_wasm::emit_support_module()?;
        Module::new(engine, &support_wasm)
            .map_err(|e| anyhow::anyhow!("support module compile failed: {:?}", e))?
    };

    // instantiate support module
    let support_instance = linker
        .instantiate_async(&mut *store, &support_module)
        .await
        .map_err(|e| anyhow::anyhow!("support module instantiate failed: {:?}", e))?;

    // 把 support module 的 12 个 helper exports 注册到 "wjsm_support" namespace
    for export_name in wjsm_runtime_support::abi::SUPPORT_EXPORTS {
        let export = support_instance
            .get_export(&mut *store, export_name)
            .ok_or_else(|| anyhow::anyhow!("support module missing export: {}", export_name))?;
        linker.define(&*store, "wjsm_support", export_name, export)?;
    }

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
    initialize_host_post_bootstrap(&mut bundle.store, &bundle.wasm_env);
    if let Ok(bootstrap_fn) = bundle
        .instance
        .get_typed_func::<(), i64>(&mut bundle.store, "__wjsm_bootstrap_once")
    {
        bootstrap_fn
            .call_async(&mut bundle.store, ())
            .await
            .map_err(|e| anyhow::anyhow!("bootstrap_once failed: {e:?}"))?;
    }
    install_array_iterator_methods(&mut bundle.store, &bundle.wasm_env);
    crate::runtime_heap::ensure_error_prototypes_initialized(&mut bundle.store, &bundle.wasm_env);
    crate::runtime_heap::ensure_symbol_prototype_initialized(&mut bundle.store, &bundle.wasm_env);
    crate::runtime_heap::ensure_promise_prototype_initialized(&mut bundle.store, &bundle.wasm_env);
    crate::runtime_heap::ensure_regexp_prototype_initialized(&mut bundle.store, &bundle.wasm_env);
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
    Ok(())
}

/// 执行 cold startup：只跑 bootstrap，不在客户机器上 capture/write snapshot。
pub(super) async fn run_startup_cold_path(bundle: &mut ExecuteInstanceBundle) -> Result<()> {
    run_bootstrap_only(bundle).await
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
    match startup_snapshot::restore_startup_snapshot(&mut bundle.store, &bundle.wasm_env, view) {
        Ok(()) => true,
        Err(e) => {
            if startup_snapshot_debug_enabled() {
                eprintln!("startup snapshot restore failed: {e:#}");
            }
            false
        }
    }
}
