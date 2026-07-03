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
    pub(crate) platform: &'static str,
    pub(crate) arch: &'static str,
    pub(crate) version: &'static str,
    pub(crate) versions: &'static [(&'static str, &'static str)],
}

impl ProcessState {
    pub(crate) fn from_options(options: &RuntimeOptions) -> Self {
        Self {
            argv: Arc::from(options.argv.clone()),
            cwd: options.cwd.clone(),
            env: Arc::from(options.env.clone()),
            pid: options.pid,
            platform: options.platform,
            arch: options.arch,
            version: options.version,
            versions: options.versions,
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
    let process_obj = alloc_host_object(caller, &env, 12);
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
    let _ = define_host_data_property_from_caller(caller, process_obj, "version", version);

    define_native_method(caller, process_obj, "cwd", NativeCallable::ProcessCwd);
    define_native_method(caller, process_obj, "exit", NativeCallable::ProcessExit);
    define_native_method(
        caller,
        process_obj,
        "nextTick",
        NativeCallable::ProcessNextTick,
    );

    let stdout = alloc_process_stream(caller, ProcessStreamKind::Stdout)?;
    let stderr = alloc_process_stream(caller, ProcessStreamKind::Stderr)?;
    let _ = define_host_data_property_from_caller(caller, process_obj, "stdout", stdout);
    let _ = define_host_data_property_from_caller(caller, process_obj, "stderr", stderr);

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
    }
    value::encode_bool(true)
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
    let obj = alloc_host_object(caller, &env, 1);
    define_native_method(
        caller,
        obj,
        "write",
        NativeCallable::ProcessStreamWrite { kind },
    );
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
