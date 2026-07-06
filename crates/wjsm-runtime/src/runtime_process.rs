use std::fmt;
use std::sync::{Arc, Mutex};

use super::*;

pub(crate) const PROCESS_NODE_VERSION: &str = "v22.0.0";
pub(crate) const PROCESS_NODE_VERSION_VALUE: &str = "22.0.0";
pub(crate) const PROCESS_VERSIONS: &[(&str, &str)] = &[
    ("node", PROCESS_NODE_VERSION_VALUE),
    ("wjsm", env!("CARGO_PKG_VERSION")),
];

#[derive(Clone, Debug)]
pub(crate) struct ProcessState {
    pub(crate) argv: Arc<[String]>,
    pub(crate) cwd: Option<String>,
    pub(crate) env: Arc<[(String, String)]>,
    pub(crate) pid: u32,
    pub(crate) ppid: u32,
    pub(crate) platform: &'static str,
    pub(crate) arch: &'static str,
    pub(crate) version: &'static str,
    pub(crate) versions: &'static [(&'static str, &'static str)],
    pub(crate) fs_read_roots: Arc<[std::path::PathBuf]>,
    pub(crate) fs_write_roots: Arc<[std::path::PathBuf]>,
    pub(crate) fs_allow_write_anywhere: bool,
}

impl ProcessState {
    pub(crate) fn from_options(options: &RuntimeOptions) -> Self {
        Self {
            argv: Arc::from(options.argv.clone()),
            cwd: options.cwd.clone(),
            env: Arc::from(options.env.clone()),
            pid: options.pid,
            ppid: options.ppid,
            platform: options.platform,
            arch: options.arch,
            version: options.version,
            versions: options.versions,
            fs_read_roots: Arc::from(options.fs_read_roots.clone()),
            fs_write_roots: Arc::from(options.fs_write_roots.clone()),
            fs_allow_write_anywhere: options.fs_allow_write_anywhere,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProcessNextTickTask {
    pub(crate) callback: i64,
    pub(crate) args: Vec<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProcessEnvTrapKind {
    Get,
    Set,
    DeleteProperty,
    DefineProperty,
    OwnKeys,
    GetOwnPropertyDescriptor,
    Has,
    PreventExtensions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProcessStreamKind {
    Stdout,
    Stderr,
    Stdin,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessExitSignal {
    pub code: i32,
    diagnostics: Vec<u8>,
}

impl ProcessExitSignal {
    pub(crate) fn new(code: i32) -> Self {
        Self {
            code,
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn with_diagnostics(mut self, diagnostics: Vec<u8>) -> Self {
        self.diagnostics = diagnostics;
        self
    }

    pub fn diagnostics(&self) -> &[u8] {
        &self.diagnostics
    }
}

impl fmt::Display for ProcessExitSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "process exited with code {}", self.code)
    }
}

impl std::error::Error for ProcessExitSignal {}

pub fn process_exit_code(error: &anyhow::Error) -> Option<i32> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<ProcessExitSignal>()
            .map(|signal| signal.code)
    })
}

pub fn process_exit_diagnostics(error: &anyhow::Error) -> Option<&[u8]> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<ProcessExitSignal>()
            .map(ProcessExitSignal::diagnostics)
    })
}

pub(crate) type ProcessNextTickQueue = Arc<Mutex<std::collections::VecDeque<ProcessNextTickTask>>>;

pub(crate) fn install_process_global_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    global_obj: i64,
) -> Option<()> {
    let env = WasmEnv::from_caller(caller)?;
    let process_obj = alloc_host_object(caller, &env, 20);
    let process = caller.data().process.clone();

    let argv = alloc_string_array(caller, &process.argv);
    let env_proxy = create_process_env_proxy(caller)?;
    let versions = alloc_versions_object(caller, process.versions);
    let platform = store_runtime_string(caller, process.platform.to_string());
    let arch = store_runtime_string(caller, process.arch.to_string());
    let version = store_runtime_string(caller, process.version.to_string());

    let _ = define_host_data_property_from_caller(caller, process_obj, "argv", argv);
    let _ = define_host_data_property_from_caller(caller, process_obj, "env", env_proxy);
    let _ = define_host_data_property_from_caller(caller, process_obj, "platform", platform);
    let _ = define_host_data_property_from_caller(caller, process_obj, "arch", arch);
    let _ = define_host_data_property_from_caller(caller, process_obj, "versions", versions);
    let _ = define_host_data_property_from_caller(
        caller,
        process_obj,
        "pid",
        value::encode_f64(process.pid as f64),
    );
    let _ = define_host_data_property_from_caller(
        caller,
        process_obj,
        "ppid",
        value::encode_f64(process.ppid as f64),
    );
    let _ = define_host_data_property_from_caller(caller, process_obj, "version", version);

    define_native_method(caller, process_obj, "cwd", NativeCallable::ProcessCwd);
    define_native_method(caller, process_obj, "exit", NativeCallable::ProcessExit);
    define_native_method(
        caller,
        process_obj,
        "nextTick",
        NativeCallable::ProcessNextTick,
    );
    define_native_method(caller, process_obj, "hrtime", NativeCallable::ProcessHrtime);
    define_native_method(
        caller,
        process_obj,
        "memoryUsage",
        NativeCallable::ProcessMemoryUsage,
    );
    define_native_method(caller, process_obj, "uptime", NativeCallable::ProcessUptime);
    define_native_method(
        caller,
        process_obj,
        "cpuUsage",
        NativeCallable::ProcessCpuUsage,
    );

    let stdin = alloc_process_stream(caller, ProcessStreamKind::Stdin)?;

    let stdout = alloc_process_stream(caller, ProcessStreamKind::Stdout)?;
    let stderr = alloc_process_stream(caller, ProcessStreamKind::Stderr)?;
    let _ = define_host_data_property_from_caller(caller, process_obj, "stdout", stdout);
    let _ = define_host_data_property_from_caller(caller, process_obj, "stderr", stderr);
    let _ = define_host_data_property_from_caller(caller, process_obj, "stdin", stdin);

    define_host_data_property_from_caller(caller, global_obj, "process", process_obj)
}

pub(crate) fn call_process_cwd(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    match caller.data().process.cwd.clone() {
        Some(cwd) => store_runtime_string(caller, cwd),
        None => make_process_error_exception(caller, "process.cwd() is unavailable"),
    }
}

pub(crate) fn call_process_exit(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let code = args
        .first()
        .copied()
        .map(|value| process_exit_code_from_value(caller, value))
        .unwrap_or(0);
    *caller
        .data()
        .process_exit_signal
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(ProcessExitSignal::new(code));
    value::encode_undefined()
}

pub(crate) fn call_process_next_tick(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let callback = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let Some(env) = WasmEnv::from_caller(caller) else {
        return make_type_error_exception(
            caller,
            "TypeError: process.nextTick requires a callable",
        );
    };
    if !is_callable_with_env(caller, &env, callback) {
        return make_type_error_exception(
            caller,
            "TypeError: process.nextTick requires a callable",
        );
    }

    caller
        .data()
        .next_tick_queue
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(ProcessNextTickTask {
            callback,
            args: args.iter().skip(1).copied().collect(),
        });
    value::encode_undefined()
}

pub(crate) fn call_process_stream_write(
    caller: &mut Caller<'_, RuntimeState>,
    kind: ProcessStreamKind,
    args: &[i64],
) -> i64 {
    let chunk = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let text = render_value(caller, chunk).unwrap_or_default();
    let bytes = text.as_bytes();
    match kind {
        ProcessStreamKind::Stdout => caller
            .data()
            .output
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(bytes),
        ProcessStreamKind::Stderr => caller
            .data()
            .diagnostics
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(bytes),
        ProcessStreamKind::Stdin => {}
    }
    value::encode_bool(true)
}

pub(crate) fn call_process_stream_end(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    kind: ProcessStreamKind,
    args: &[i64],
) -> i64 {
    if args
        .first()
        .is_some_and(|chunk| !value::is_undefined(*chunk))
    {
        let _ = call_process_stream_write(caller, kind, args);
    }
    this_val
}

pub(crate) fn call_process_stream_on(
    _caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    _kind: ProcessStreamKind,
    _args: &[i64],
) -> i64 {
    this_val
}

pub(crate) fn call_process_stdin_resume(
    _caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> i64 {
    this_val
}

pub(crate) fn call_process_hrtime(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let elapsed = caller.data().performance_origin.elapsed();
    let mut seconds = elapsed.as_secs() as i64;
    let mut nanos = elapsed.subsec_nanos() as i64;
    if let Some((prev_seconds, prev_nanos)) = args
        .first()
        .copied()
        .and_then(|value| read_hrtime_pair(caller, value))
    {
        seconds -= prev_seconds;
        nanos -= prev_nanos;
        if nanos < 0 {
            seconds -= 1;
            nanos += 1_000_000_000;
        }
        if seconds < 0 {
            seconds = 0;
            nanos = 0;
        }
    }
    let arr = alloc_array(caller, 2);
    set_array_elem(caller, arr, 0, value::encode_f64(seconds as f64));
    set_array_elem(caller, arr, 1, value::encode_f64(nanos as f64));
    arr
}

pub(crate) fn call_process_hrtime_bigint(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let elapsed = caller.data().performance_origin.elapsed();
    let nanos = elapsed.as_nanos();
    let mut table = caller
        .data()
        .bigint_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let handle = table.len() as u32;
    table.push(num_bigint::BigInt::from(nanos));
    value::encode_bigint_handle(handle)
}

pub(crate) fn call_process_uptime(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    value::encode_f64(caller.data().performance_origin.elapsed().as_secs_f64())
}

pub(crate) fn call_process_memory_usage(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let heap_used = heap_used_bytes(caller, &env) as f64;
    let heap_total = env.memory.size(&mut *caller) as f64 * 65_536.0;
    let array_buffers = caller
        .data()
        .arraybuffer_table
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|entry| entry.data.len() as u64)
        .sum::<u64>() as f64;
    let obj = alloc_host_object(caller, &env, 5);
    let _ = define_host_data_property_from_caller(caller, obj, "rss", value::encode_f64(0.0));
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "heapTotal",
        value::encode_f64(heap_total),
    );
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "heapUsed",
        value::encode_f64(heap_used),
    );
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "external",
        value::encode_f64(array_buffers),
    );
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "arrayBuffers",
        value::encode_f64(array_buffers),
    );
    obj
}

pub(crate) fn call_process_cpu_usage(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let (mut user, mut system) = current_cpu_usage_micros(caller);
    if let Some((prev_user, prev_system)) = args
        .first()
        .copied()
        .and_then(|value| read_cpu_usage_pair(caller, value))
    {
        user = user.saturating_sub(prev_user);
        system = system.saturating_sub(prev_system);
    }
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    let _ =
        define_host_data_property_from_caller(caller, obj, "user", value::encode_f64(user as f64));
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "system",
        value::encode_f64(system as f64),
    );
    obj
}

pub(crate) fn call_process_env_trap(
    caller: &mut Caller<'_, RuntimeState>,
    kind: ProcessEnvTrapKind,
    args: &[i64],
) -> i64 {
    match kind {
        ProcessEnvTrapKind::Get => {
            let Some(key) = args
                .get(1)
                .copied()
                .and_then(|prop| process_env_key(caller, prop))
            else {
                return value::encode_undefined();
            };
            env_value(caller, &key)
                .map(|value| store_runtime_string(caller, value))
                .unwrap_or_else(value::encode_undefined)
        }
        ProcessEnvTrapKind::Has => {
            let exists = args
                .get(1)
                .copied()
                .and_then(|prop| process_env_key(caller, prop))
                .is_some_and(|key| env_value(caller, &key).is_some());
            value::encode_bool(exists)
        }
        ProcessEnvTrapKind::OwnKeys => {
            let keys: Vec<String> = caller
                .data()
                .process
                .env
                .iter()
                .map(|(key, _)| key.clone())
                .collect();
            alloc_string_array(caller, &keys)
        }
        ProcessEnvTrapKind::GetOwnPropertyDescriptor => {
            let Some(key) = args
                .get(1)
                .copied()
                .and_then(|prop| process_env_key(caller, prop))
            else {
                return value::encode_undefined();
            };
            let Some(value) = env_value(caller, &key) else {
                return value::encode_undefined();
            };
            let val = store_runtime_string(caller, value);
            allocate_descriptor_object(
                caller,
                false,
                val,
                false,
                true,
                true,
                value::encode_undefined(),
                value::encode_undefined(),
            )
            .unwrap_or_else(value::encode_undefined)
        }
        ProcessEnvTrapKind::Set
        | ProcessEnvTrapKind::DeleteProperty
        | ProcessEnvTrapKind::DefineProperty => value::encode_bool(false),
        ProcessEnvTrapKind::PreventExtensions => value::encode_bool(true),
    }
}

pub(crate) fn take_process_exit_signal(state: &RuntimeState) -> Option<ProcessExitSignal> {
    state
        .process_exit_signal
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

pub(crate) fn pending_process_exit_signal(state: &RuntimeState) -> Option<ProcessExitSignal> {
    state
        .process_exit_signal
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn define_native_method(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    callable: NativeCallable,
) {
    let val = create_process_native_callable(caller, callable);
    let _ = define_host_data_property_from_caller(caller, obj, name, val);
}

fn alloc_process_stream(
    caller: &mut Caller<'_, RuntimeState>,
    kind: ProcessStreamKind,
) -> Option<i64> {
    let env = WasmEnv::from_caller(caller)?;
    let obj = alloc_host_object(caller, &env, 5);
    match kind {
        ProcessStreamKind::Stdout | ProcessStreamKind::Stderr => {
            define_native_method(
                caller,
                obj,
                "write",
                NativeCallable::ProcessStreamWrite { kind },
            );
            define_native_method(
                caller,
                obj,
                "end",
                NativeCallable::ProcessStreamEnd { kind },
            );
            define_native_method(caller, obj, "on", NativeCallable::ProcessStreamOn { kind });
        }
        ProcessStreamKind::Stdin => {
            define_native_method(caller, obj, "on", NativeCallable::ProcessStreamOn { kind });
            define_native_method(caller, obj, "resume", NativeCallable::ProcessStdinResume);
            let _ = define_host_data_property_from_caller(
                caller,
                obj,
                "readable",
                value::encode_bool(true),
            );
            let _ = define_host_data_property_from_caller(
                caller,
                obj,
                "isTTY",
                value::encode_bool(false),
            );
        }
    }
    Some(obj)
}

fn create_process_env_proxy(caller: &mut Caller<'_, RuntimeState>) -> Option<i64> {
    let env = WasmEnv::from_caller(caller)?;
    let env_snapshot = caller.data().process.env.clone();
    let target = alloc_host_object(caller, &env, env_snapshot.len() as u32);
    for (key, value) in env_snapshot.iter() {
        let val = store_runtime_string(caller, value.clone());
        let name_id =
            find_memory_c_string(caller, key).or_else(|| alloc_heap_c_string(caller, key))?;
        let _ = define_host_data_property_by_name_id_with_flags(
            caller,
            target,
            encode_string_name_id(name_id),
            val,
            constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE,
        );
    }
    prevent_extensions_impl(caller, target);

    let handler = alloc_host_object(caller, &env, 8);
    attach_env_trap(caller, handler, "get", ProcessEnvTrapKind::Get);
    attach_env_trap(caller, handler, "set", ProcessEnvTrapKind::Set);
    attach_env_trap(
        caller,
        handler,
        "deleteProperty",
        ProcessEnvTrapKind::DeleteProperty,
    );
    attach_env_trap(
        caller,
        handler,
        "defineProperty",
        ProcessEnvTrapKind::DefineProperty,
    );
    attach_env_trap(caller, handler, "ownKeys", ProcessEnvTrapKind::OwnKeys);
    attach_env_trap(
        caller,
        handler,
        "getOwnPropertyDescriptor",
        ProcessEnvTrapKind::GetOwnPropertyDescriptor,
    );
    attach_env_trap(caller, handler, "has", ProcessEnvTrapKind::Has);
    attach_env_trap(
        caller,
        handler,
        "preventExtensions",
        ProcessEnvTrapKind::PreventExtensions,
    );

    let handle = {
        let mut table = caller
            .data()
            .proxy_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(ProxyEntry {
            target,
            handler,
            revoked: false,
        });
        handle
    };
    Some(value::encode_proxy_handle(handle))
}

fn attach_env_trap(
    caller: &mut Caller<'_, RuntimeState>,
    handler: i64,
    name: &str,
    kind: ProcessEnvTrapKind,
) {
    define_native_method(
        caller,
        handler,
        name,
        NativeCallable::ProcessEnvTrap { kind },
    );
}

fn alloc_versions_object(
    caller: &mut Caller<'_, RuntimeState>,
    versions: &[(&'static str, &'static str)],
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, versions.len() as u32);
    for (name, value) in versions {
        let val = store_runtime_string(caller, (*value).to_string());
        let _ = define_host_data_property_from_caller(caller, obj, name, val);
    }
    obj
}

fn alloc_string_array(caller: &mut Caller<'_, RuntimeState>, values: &[String]) -> i64 {
    let arr = alloc_array(caller, values.len() as u32);
    for (index, item) in values.iter().enumerate() {
        let value = store_runtime_string(caller, item.clone());
        set_array_elem(caller, arr, index as i32, value);
    }
    arr
}

fn create_process_native_callable(
    caller: &mut Caller<'_, RuntimeState>,
    callable: NativeCallable,
) -> i64 {
    let free_slot = caller
        .data()
        .native_callable_free_slots
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pop();
    let mut table = caller
        .data()
        .native_callables
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let idx = if let Some(idx) = free_slot {
        if let Some(slot) = table.get_mut(idx as usize) {
            *slot = callable;
            idx
        } else {
            let idx = table.len() as u32;
            table.push(callable);
            idx
        }
    } else {
        let idx = table.len() as u32;
        table.push(callable);
        idx
    };
    value::encode_native_callable_idx(idx)
}

fn read_hrtime_pair(caller: &mut Caller<'_, RuntimeState>, value_raw: i64) -> Option<(i64, i64)> {
    let ptr = value::is_array(value_raw)
        .then(|| resolve_array_ptr(caller, value_raw))
        .flatten()?;
    let seconds = read_array_elem(caller, ptr, 0)?;
    let nanos = read_array_elem(caller, ptr, 1)?;
    Some((
        value::decode_f64(to_number(caller, seconds)).trunc() as i64,
        value::decode_f64(to_number(caller, nanos)).trunc() as i64,
    ))
}

fn read_cpu_usage_pair(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
) -> Option<(u64, u64)> {
    let ptr = value::is_object(value_raw)
        .then(|| resolve_handle(caller, value_raw))
        .flatten()?;
    let user = read_object_property_by_name(caller, ptr, "user")?;
    let system = read_object_property_by_name(caller, ptr, "system")?;
    Some((
        value::decode_f64(to_number(caller, user)).max(0.0).trunc() as u64,
        value::decode_f64(to_number(caller, system))
            .max(0.0)
            .trunc() as u64,
    ))
}

fn current_cpu_usage_micros(caller: &mut Caller<'_, RuntimeState>) -> (u64, u64) {
    #[cfg(unix)]
    {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: `getrusage` initializes `usage` when it returns 0, and the pointer is valid.
        let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
        if rc == 0 {
            // SAFETY: rc == 0 means libc initialized the rusage struct.
            let usage = unsafe { usage.assume_init() };
            return (
                timeval_to_micros(usage.ru_utime),
                timeval_to_micros(usage.ru_stime),
            );
        }
    }
    (
        caller.data().performance_origin.elapsed().as_micros() as u64,
        0,
    )
}

#[cfg(unix)]
fn timeval_to_micros(value: libc::timeval) -> u64 {
    (value.tv_sec as u64)
        .saturating_mul(1_000_000)
        .saturating_add(value.tv_usec as u64)
}

fn process_env_key(caller: &mut Caller<'_, RuntimeState>, prop: i64) -> Option<String> {
    if value::is_symbol(prop) {
        return None;
    }
    render_value(caller, prop).ok()
}

fn env_value(caller: &Caller<'_, RuntimeState>, key: &str) -> Option<String> {
    caller
        .data()
        .process
        .env
        .iter()
        .find_map(|(name, value)| (name == key).then(|| value.clone()))
}

fn make_process_error_exception(caller: &mut Caller<'_, RuntimeState>, msg: &str) -> i64 {
    let msg_val = store_runtime_string(caller, msg.to_string());
    let error_obj = create_error_object(caller, "Error", msg_val, value::encode_undefined());
    let mut errors = caller
        .data()
        .error_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let idx = errors.len() as u32;
    errors.push(ErrorEntry {
        name: "Error".to_string(),
        message: msg.to_string(),
        value: error_obj,
    });
    value::encode_handle(value::TAG_EXCEPTION, idx)
}

fn process_exit_code_from_value(caller: &mut Caller<'_, RuntimeState>, value: i64) -> i32 {
    if value::is_undefined(value) {
        return 0;
    }
    let number = value::decode_f64(to_number(caller, value));
    if !number.is_finite() || number == 0.0 {
        return 0;
    }
    let int = number.trunc();
    let modulo = int.rem_euclid(4_294_967_296.0);
    let signed = if modulo >= 2_147_483_648.0 {
        modulo - 4_294_967_296.0
    } else {
        modulo
    };
    signed as i32
}
