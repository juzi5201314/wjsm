//! Worker 创建、OS 线程生命周期、编译与 host 方法。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use wasmtime::{Caller, Store};

use crate::runtime_encoding::js_string_lossy;
use crate::runtime_node_data::object_bool_property;
use crate::runtime_process::ProcessNextTickTask;
use crate::runtime_values::{read_object_property_by_name, resolve_handle};
use crate::runtime_worker_message::{
    SerializedValue, deserialize_value, serialize_for_post_message,
};
use crate::scheduler::AsyncHostCompletion;
use crate::shared_buffer::SharedRuntimeState;
use crate::*;

use super::cluster::{LocalWorkerBinding, WorkerClusterState, WorkerControl, cluster_of};
use super::port::{
    ensure_local_port, host_object_two_ids, port_id_arg, port_post_message, port_start,
};

pub(super) fn create_worker(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let filename_raw = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let filename = js_string_lossy(caller, filename_raw);
    let options = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let is_eval = if value::is_object(options) {
        object_bool_property(caller, options, "eval").unwrap_or(false)
    } else {
        false
    };
    let worker_data = if value::is_object(options) {
        resolve_handle(caller, options)
            .and_then(|ptr| read_object_property_by_name(caller, ptr, "workerData"))
            .unwrap_or_else(value::encode_undefined)
    } else {
        value::encode_undefined()
    };
    let transfer = if value::is_object(options) {
        resolve_handle(caller, options)
            .and_then(|ptr| read_object_property_by_name(caller, ptr, "transferList"))
            .unwrap_or_else(value::encode_undefined)
    } else {
        value::encode_undefined()
    };
    let serialized_data = match serialize_for_post_message(caller, worker_data, transfer) {
        Ok(v) => v,
        Err(msg) => return make_type_error_exception(caller, &msg),
    };
    let Some(shared) = caller.data().shared_state.clone() else {
        return make_type_error_exception(caller, "worker_threads shared state missing");
    };
    let cluster = Arc::clone(&shared.worker_cluster);
    let max = cluster.max_workers.load(Ordering::Relaxed);
    let active = cluster.active_workers.load(Ordering::Relaxed);
    if active >= max {
        return make_type_error_exception(
            caller,
            &format!("ERR_WORKER_INIT_FAILED: max workers ({max}) exceeded"),
        );
    }
    let (parent_port, worker_port) = cluster.alloc_port_pair();
    let worker_id = cluster.next_worker_id.fetch_add(1, Ordering::Relaxed);
    let thread_id = cluster.next_thread_id.fetch_add(1, Ordering::Relaxed);
    let control = Arc::new(WorkerControl {
        id: worker_id,
        thread_id,
        parent_port_id: parent_port.id,
        worker_port_id: worker_port.id,
        terminated: AtomicBool::new(false),
        exit_notified: AtomicBool::new(false),
        worker_wake_tx: Mutex::new(None),
        parent_wake_tx: Mutex::new(caller.data().host_completion_tx.clone()),
    });
    cluster
        .workers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(worker_id, Arc::clone(&control));
    cluster.active_workers.fetch_add(1, Ordering::SeqCst);
    ensure_local_port(caller, parent_port.id);
    let worker_scope = crate::runtime_async_hooks::capture_from_caller(caller);
    {
        let mut map = caller
            .data()
            .worker_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let lifetime_guard = caller.data().async_op_counter.as_ref().map(|c| c.begin());
        map.insert(
            worker_id,
            LocalWorkerBinding {
                global_id: worker_id,
                online_cb: None,
                message_cb: None,
                error_cb: None,
                exit_cb: None,
                lifetime_guard,
                scope: worker_scope,
            },
        );
    }
    let parent_options = clone_runtime_options_for_worker(caller);
    let wasm_result =
        compile_worker_source(&filename, is_eval, caller.data().process.cwd.as_deref());
    let wasm_bytes = match wasm_result {
        Ok(bytes) => bytes,
        Err(err) => {
            cluster.active_workers.fetch_sub(1, Ordering::SeqCst);
            return make_type_error_exception(caller, &format!("ERR_WORKER_INIT_FAILED: {err}"));
        }
    };
    spawn_worker_thread(
        shared,
        Arc::clone(&control),
        wasm_bytes,
        parent_options,
        serialized_data,
        worker_port.id,
        thread_id,
    );
    host_object_two_ids(caller, "id", worker_id, "threadId", thread_id)
}

fn clone_runtime_options_for_worker(caller: &Caller<'_, RuntimeState>) -> RuntimeOptions {
    let process = &caller.data().process;
    RuntimeOptions {
        max_heap_size: caller.data().max_heap_size,
        shadow_stack_max: caller.data().shadow_stack_max(),
        wasmtime_memory_reservation: None,
        gc_algorithm: crate::runtime_gc::GcAlgorithmKind::MarkSweep,
        // worker 继承父 agent 的显式编译器选择。
        compiler: caller.data().compiler,
        current_entry: caller.data().current_entry.clone(),
        argv: process.argv.iter().cloned().collect(),
        cwd: process.cwd.clone(),
        env: process.env.iter().cloned().collect(),
        exec_path: Some(process.exec_path.clone()),
        exec_argv: process.exec_argv.iter().cloned().collect(),
        pid: process.pid,
        ppid: process.ppid,
        platform: process.platform,
        arch: process.arch,
        version: process.version,
        versions: process.versions,
        fs_read_roots: process.fs_read_roots.iter().cloned().collect(),
        fs_write_roots: process.fs_write_roots.iter().cloned().collect(),
        fs_allow_write_anywhere: process.fs_allow_write_anywhere,
        module_loader: caller.data().module_loader.clone(),
        is_worker_thread: true,
        worker_thread_id: 0,
        parent_port_global_id: None,
        initial_worker_data: None,
        // Worker 不继承主线程 CDP 服务；需要时由 Worker 独立 `--inspect` 启动。
        inspect: None,
    }
}

fn compile_worker_source(
    filename: &str,
    is_eval: bool,
    cwd: Option<&str>,
) -> anyhow::Result<Vec<u8>> {
    if is_eval {
        return compile_eval_source(filename);
    }
    let path = PathBuf::from(filename);
    let abs = if path.is_absolute() {
        path
    } else if let Some(cwd) = cwd {
        Path::new(cwd).join(path)
    } else {
        std::env::current_dir()?.join(path)
    };
    let root = abs
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    wjsm_module::bundle_with_options(&abs, &root, wjsm_module::ResolutionOptions::default())
}

fn compile_eval_source(source: &str) -> anyhow::Result<Vec<u8>> {
    let dir = std::env::temp_dir().join(format!(
        "wjsm_worker_eval_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("eval_worker.js");
    std::fs::write(&path, source)?;
    let result =
        wjsm_module::bundle_with_options(&path, &dir, wjsm_module::ResolutionOptions::default());
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn spawn_worker_thread(
    shared: Arc<SharedRuntimeState>,
    control: Arc<WorkerControl>,
    wasm_bytes: Vec<u8>,
    mut options: RuntimeOptions,
    worker_data: SerializedValue,
    parent_port_id: u32,
    thread_id: u32,
) {
    options.is_worker_thread = true;
    options.worker_thread_id = thread_id;
    options.parent_port_global_id = Some(parent_port_id);
    options.initial_worker_data = Some(worker_data);
    std::thread::Builder::new()
        .name(format!("wjsm-worker-{}", control.id))
        .spawn(move || {
            run_worker_thread(shared, control, wasm_bytes, options);
        })
        .expect("spawn worker thread");
}

fn run_worker_thread(
    shared: Arc<SharedRuntimeState>,
    control: Arc<WorkerControl>,
    wasm_bytes: Vec<u8>,
    options: RuntimeOptions,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            notify_worker_error_and_exit(&control, format!("worker tokio runtime: {e}"), 1);
            shared
                .worker_cluster
                .active_workers
                .fetch_sub(1, Ordering::SeqCst);
            return;
        }
    };
    let outcome = rt.block_on(async {
        // online：JS 即将开始执行
        notify_worker_online(&control);
        let mut out = Vec::new();
        let result = crate::execute_with_writer_shared_agent_options(
            &wasm_bytes,
            &mut out,
            shared.clone(),
            options,
        )
        .await;
        result.map(|_| ())
    });
    match outcome {
        Ok(()) => {
            if !control.terminated.load(Ordering::SeqCst) {
                notify_worker_exit(&control, 0);
            } else {
                notify_worker_exit(&control, 1);
            }
        }
        Err(err) => {
            if let Some(code) = crate::runtime_process::process_exit_code(&err) {
                notify_worker_exit(&control, code);
            } else {
                let message = extract_error_message(&err);
                notify_worker_error_and_exit(&control, message, 1);
            }
        }
    }
    shared
        .worker_cluster
        .active_workers
        .fetch_sub(1, Ordering::SeqCst);
}

fn extract_error_message(err: &anyhow::Error) -> String {
    let full = format!("{err:#}");
    let rest = full
        .strip_prefix("Uncaught exception: ")
        .unwrap_or(full.as_str());
    // error_table 常把 "Error: msg" 整段写入 message；Worker 'error' 事件要裸 message。
    rest.strip_prefix("Error: ")
        .or_else(|| rest.strip_prefix("TypeError: "))
        .or_else(|| rest.strip_prefix("RangeError: "))
        .unwrap_or(rest)
        .to_string()
}

fn notify_worker_online(control: &WorkerControl) {
    let worker_id = control.id;
    send_parent_host_task(control, move |store, _env| {
        invoke_lifecycle_cb(store, worker_id, LifecycleKind::Online, None);
    });
}

fn notify_worker_exit(control: &WorkerControl, code: i32) {
    if control.exit_notified.swap(true, Ordering::SeqCst) {
        return;
    }
    let worker_id = control.id;
    send_parent_host_task(control, move |store, _env| {
        invoke_lifecycle_cb(store, worker_id, LifecycleKind::Exit, Some(code as f64));
        clear_worker_lifetime(store, worker_id);
    });
}

fn notify_worker_error_and_exit(control: &WorkerControl, message: String, code: i32) {
    let worker_id = control.id;
    send_parent_host_task(control, move |store, env| {
        let err_val =
            crate::runtime_heap::alloc_error_object_with_env(store, env, "Error", message, None);
        invoke_lifecycle_cb_value(store, worker_id, LifecycleKind::Error, err_val);
    });
    notify_worker_exit(control, code);
}

enum LifecycleKind {
    Online,
    #[allow(dead_code)]
    Message,
    Error,
    Exit,
}

fn send_parent_host_task<F>(control: &WorkerControl, f: F)
where
    F: FnOnce(&mut Store<RuntimeState>, &WasmEnv) + Send + 'static,
{
    let tx = control
        .parent_wake_tx
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(tx) = tx {
        let _ = tx.send(AsyncHostCompletion::HostTask {
            run: Box::new(f),
            scope: None,
        });
    }
}

fn invoke_lifecycle_cb(
    store: &mut Store<RuntimeState>,
    worker_id: u32,
    kind: LifecycleKind,
    number_arg: Option<f64>,
) {
    let Some((cb, scope)) = lifecycle_cb(store, worker_id, &kind) else {
        return;
    };
    let args = match number_arg {
        Some(n) => vec![value::encode_f64(n)],
        None => Vec::new(),
    };
    store
        .data()
        .next_tick_queue
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(ProcessNextTickTask {
            callback: cb,
            args,
            scope,
        });
}

fn invoke_lifecycle_cb_value(
    store: &mut Store<RuntimeState>,
    worker_id: u32,
    kind: LifecycleKind,
    arg: i64,
) {
    let Some((cb, scope)) = lifecycle_cb(store, worker_id, &kind) else {
        return;
    };
    store
        .data()
        .next_tick_queue
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(ProcessNextTickTask {
            callback: cb,
            args: vec![arg],
            scope,
        });
}

fn lifecycle_cb(
    store: &Store<RuntimeState>,
    worker_id: u32,
    kind: &LifecycleKind,
) -> Option<(i64, Option<crate::CapturedScope>)> {
    let map = store
        .data()
        .worker_bindings
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let binding = map.get(&worker_id)?;
    let callback = match kind {
        LifecycleKind::Online => binding.online_cb,
        LifecycleKind::Message => binding.message_cb,
        LifecycleKind::Error => binding.error_cb,
        LifecycleKind::Exit => binding.exit_cb,
    }?;
    Some((callback, binding.scope))
}

fn clear_worker_lifetime(store: &mut Store<RuntimeState>, worker_id: u32) {
    if let Some(binding) = store
        .data()
        .worker_bindings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(&worker_id)
    {
        binding.lifetime_guard = None;
    }
}

pub(super) fn worker_post_message(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(worker_id) = port_id_arg(args, 0) else {
        return make_type_error_exception(caller, "workerPostMessage: invalid worker id");
    };
    let Some(cluster) = cluster_of(caller) else {
        return value::encode_undefined();
    };
    let Some(control) = cluster.worker(worker_id) else {
        return value::encode_undefined();
    };
    // 发往 worker 的 parentPort：payload 进入 worker_port inbox
    let mut forward = vec![value::encode_f64(control.parent_port_id as f64)];
    if let Some(v) = args.get(1) {
        forward.push(*v);
    }
    if let Some(t) = args.get(2) {
        forward.push(*t);
    }
    port_post_message(caller, &forward)
}

pub(super) fn worker_terminate(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(worker_id) = port_id_arg(args, 0) else {
        return value::encode_undefined();
    };
    let Some(cluster) = cluster_of(caller) else {
        return value::encode_undefined();
    };
    let Some(control) = cluster.worker(worker_id) else {
        return value::encode_undefined();
    };
    control.terminated.store(true, Ordering::SeqCst);
    close_port_endpoint(cluster.as_ref(), control.parent_port_id);
    close_port_endpoint(cluster.as_ref(), control.worker_port_id);
    // 向 worker scheduler 注入 process.exit(1)
    let tx = control
        .worker_wake_tx
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(tx) = tx {
        let _ = tx.send(AsyncHostCompletion::HostTask {
            run: Box::new(|store, _env| {
                *store
                    .data()
                    .process_exit_signal
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) =
                    Some(crate::runtime_process::ProcessExitSignal::new(1));
            }),
            scope: None,
        });
    } else {
        // worker 可能已结束：直接 exit 通知
        notify_worker_exit(&control, 1);
        clear_worker_lifetime_caller(caller, worker_id);
    }
    value::encode_undefined()
}

fn close_port_endpoint(cluster: &WorkerClusterState, port_id: u32) {
    if let Some(port) = cluster.port(port_id) {
        port.closed.store(true, Ordering::SeqCst);
        port.inbox.lock().unwrap_or_else(|e| e.into_inner()).clear();
        *port.wake_tx.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

fn clear_worker_lifetime_caller(caller: &mut Caller<'_, RuntimeState>, worker_id: u32) {
    if let Some(binding) = caller
        .data()
        .worker_bindings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(&worker_id)
    {
        binding.lifetime_guard = None;
    }
}

pub(super) fn worker_ref(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(worker_id) = port_id_arg(args, 0) else {
        return value::encode_undefined();
    };
    let mut map = caller
        .data()
        .worker_bindings
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(binding) = map.get_mut(&worker_id)
        && binding.lifetime_guard.is_none()
        && let Some(counter) = caller.data().async_op_counter.clone()
    {
        binding.lifetime_guard = Some(counter.begin());
    }
    value::encode_undefined()
}

pub(super) fn worker_unref(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(worker_id) = port_id_arg(args, 0) else {
        return value::encode_undefined();
    };
    if let Some(binding) = caller
        .data()
        .worker_bindings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(&worker_id)
    {
        binding.lifetime_guard = None;
    }
    value::encode_undefined()
}

pub(super) fn worker_on_lifecycle(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(worker_id) = port_id_arg(args, 0) else {
        return value::encode_undefined();
    };
    let handlers = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let (online, message, error, exit) = read_lifecycle_handlers(caller, handlers);
    {
        let mut map = caller
            .data()
            .worker_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(binding) = map.get_mut(&worker_id) {
            binding.online_cb = online;
            binding.message_cb = message;
            binding.error_cb = error;
            binding.exit_cb = exit;
        }
    }
    // 父端 port 自动 start，将 worker postMessage 映射到 Worker 'message'
    if let Some(cluster) = cluster_of(caller)
        && let Some(control) = cluster.worker(worker_id)
    {
        let parent_port = control.parent_port_id;
        if let Some(message_cb) = message {
            let start_args = [value::encode_f64(parent_port as f64), message_cb];
            let _ = port_start(caller, &start_args);
        }
        // 刷新 parent_wake_tx
        *control
            .parent_wake_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = caller.data().host_completion_tx.clone();
    }
    value::encode_undefined()
}

fn read_lifecycle_handlers(
    caller: &mut Caller<'_, RuntimeState>,
    handlers: i64,
) -> (Option<i64>, Option<i64>, Option<i64>, Option<i64>) {
    if !value::is_object(handlers) {
        return (None, None, None, None);
    }
    let Some(ptr) = resolve_handle(caller, handlers) else {
        return (None, None, None, None);
    };
    let online = read_object_property_by_name(caller, ptr, "online");
    let message = read_object_property_by_name(caller, ptr, "message");
    let error = read_object_property_by_name(caller, ptr, "error");
    let exit = read_object_property_by_name(caller, ptr, "exit");
    (
        online.filter(|v| !value::is_undefined(*v) && !value::is_null(*v)),
        message.filter(|v| !value::is_undefined(*v) && !value::is_null(*v)),
        error.filter(|v| !value::is_undefined(*v) && !value::is_null(*v)),
        exit.filter(|v| !value::is_undefined(*v) && !value::is_null(*v)),
    )
}

pub(super) fn get_is_main_thread(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    value::encode_bool(!caller.data().is_worker_thread)
}

pub(super) fn get_thread_id(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    value::encode_f64(caller.data().thread_id as f64)
}

pub(super) fn get_worker_data(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let Some(data) = caller.data().worker_data_serialized.clone() else {
        return value::encode_undefined();
    };
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    deserialize_value(caller, &env, &data)
}

pub(super) fn get_parent_port_id(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    match caller.data().parent_port_id {
        Some(id) => value::encode_f64(id as f64),
        None => value::encode_null(),
    }
}

/// 在 worker Store 创建后注册 worker_port 的 wake_tx，供 parent postMessage 唤醒。
pub(crate) fn register_worker_port_wake(
    store: &mut Store<RuntimeState>,
    parent_port_id: u32,
    worker_id_hint: Option<u32>,
) {
    let Some(shared) = store.data().shared_state.clone() else {
        return;
    };
    let cluster = &shared.worker_cluster;
    if let Some(port) = cluster.port(parent_port_id) {
        *port.wake_tx.lock().unwrap_or_else(|e| e.into_inner()) =
            store.data().host_completion_tx.clone();
    }
    // 回写 worker_wake_tx 到 WorkerControl（按 parent_port 匹配）
    let workers = cluster.workers.lock().unwrap_or_else(|e| e.into_inner());
    for control in workers.values() {
        if control.worker_port_id == parent_port_id
            || worker_id_hint == Some(control.id)
            || control.worker_port_id == store.data().parent_port_id.unwrap_or(u32::MAX)
        {
            *control
                .worker_wake_tx
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = store.data().host_completion_tx.clone();
        }
    }
}
