use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use wait_timeout::ChildExt;
use wasmtime::Caller;

use crate::runtime_buffer::create_buffer_from_bytes;
use crate::runtime_encoding::js_string_lossy;
use crate::runtime_node_data::{
    bytes_from_value, object_bool_property, object_number_property, object_string_property,
    string_array_from_value,
};
use crate::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ChildProcessMethodKind {
    SpawnSync,
    ExecSync,
}

impl ChildProcessMethodKind {
    pub(crate) fn from_method(method: u8) -> Option<Self> {
        match method {
            0 => Some(Self::SpawnSync),
            1 => Some(Self::ExecSync),
            _ => None,
        }
    }

    pub(crate) fn method(self) -> u8 {
        match self {
            Self::SpawnSync => 0,
            Self::ExecSync => 1,
        }
    }
}

#[derive(Debug)]
struct CommandOptions {
    cwd: Option<String>,
    env_pairs: Vec<(String, String)>,
    shell: bool,
    timeout_ms: Option<u64>,
    max_buffer: usize,
    input: Vec<u8>,
}

#[derive(Debug)]
struct CommandOutcome {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: Option<i32>,
    signal: Option<String>,
}

pub(crate) fn create_child_process_host_object(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    let temp_root_len = caller.data().push_host_temp_roots([obj]);
    install_child_process_method(
        caller,
        obj,
        "spawnSync",
        ChildProcessMethodKind::SpawnSync,
    );
    install_child_process_method(caller, obj, "execSync", ChildProcessMethodKind::ExecSync);
    caller.data().truncate_host_temp_roots(temp_root_len);
    obj
}

pub(crate) fn call_child_process_method(
    caller: &mut Caller<'_, RuntimeState>,
    kind: ChildProcessMethodKind,
    args: &[i64],
) -> i64 {
    match kind {
        ChildProcessMethodKind::SpawnSync => spawn_sync(caller, args),
        ChildProcessMethodKind::ExecSync => exec_sync(caller, args),
    }
}

fn install_child_process_method(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    kind: ChildProcessMethodKind,
) {
    let callable = create_native_callable(caller.data(), NativeCallable::ChildProcessMethod { kind });
    let _ = define_host_data_property_from_caller(caller, obj, name, callable);
}

fn spawn_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(command) = args.first().copied() else {
        return make_type_error_exception(caller, "spawnSync command is required");
    };
    let command = js_string_lossy(caller, command);
    let spawn_args = match args.get(1).copied() {
        Some(value) => match string_array_from_value(caller, value) {
            Ok(values) => values,
            Err(error) => return error,
        },
        None => Vec::new(),
    };
    let options = match parse_options(caller, args.get(2).copied()) {
        Ok(options) => options,
        Err(error) => return error,
    };
    match run_command(caller, &command, &spawn_args, &options) {
        Ok(outcome) => create_spawn_result(caller, outcome),
        Err(error) => make_child_process_error(caller, &error),
    }
}

fn exec_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(command) = args.first().copied() else {
        return make_type_error_exception(caller, "execSync command is required");
    };
    let command = js_string_lossy(caller, command);
    let mut options = match parse_options(caller, args.get(1).copied()) {
        Ok(options) => options,
        Err(error) => return error,
    };
    options.shell = true;
    match run_command(caller, &command, &[], &options) {
        Ok(outcome) if outcome.status == Some(0) => create_buffer_from_bytes(caller, outcome.stdout),
        Ok(outcome) => make_child_process_error(
            caller,
            &format!(
                "Command failed with status {}: {command}",
                outcome.status.unwrap_or(-1)
            ),
        ),
        Err(error) => make_child_process_error(caller, &error),
    }
}

fn parse_options(
    caller: &mut Caller<'_, RuntimeState>,
    raw: Option<i64>,
) -> Result<CommandOptions, i64> {
    let raw = raw.unwrap_or_else(value::encode_undefined);
    let cwd = if value::is_object(raw) {
        object_string_property(caller, raw, "cwd")
    } else {
        None
    };
    let env_pairs = if value::is_object(raw) {
        parse_env_pairs(caller, raw)?
    } else {
        Vec::new()
    };
    let shell = if value::is_object(raw) {
        object_bool_property(caller, raw, "shell").unwrap_or(false)
    } else {
        false
    };
    let timeout_ms = if value::is_object(raw) {
        object_number_property(caller, raw, "timeout")
            .filter(|value| value.is_finite() && *value > 0.0)
            .map(|value| value as u64)
    } else {
        None
    };
    let max_buffer = if value::is_object(raw) {
        object_number_property(caller, raw, "maxBuffer")
            .filter(|value| value.is_finite() && *value > 0.0)
            .map(|value| value as usize)
            .unwrap_or(1024 * 1024)
    } else {
        1024 * 1024
    };
    let input = if value::is_object(raw) {
        let Some(ptr) = resolve_handle(caller, raw) else {
            return Ok(CommandOptions {
                cwd,
                env_pairs,
                shell,
                timeout_ms,
                max_buffer,
                input: Vec::new(),
            });
        };
        match read_object_property_by_name(caller, ptr, "input") {
            Some(input) if !value::is_undefined(input) && !value::is_null(input) => {
                bytes_from_value(caller, input, "child_process input")?
            }
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };
    Ok(CommandOptions {
        cwd,
        env_pairs,
        shell,
        timeout_ms,
        max_buffer,
        input,
    })
}

fn parse_env_pairs(
    caller: &mut Caller<'_, RuntimeState>,
    raw: i64,
) -> Result<Vec<(String, String)>, i64> {
    let Some(ptr) = resolve_handle(caller, raw) else {
        return Ok(Vec::new());
    };
    let env_pairs = read_object_property_by_name(caller, ptr, "envPairs")
        .unwrap_or_else(value::encode_undefined);
    let pairs = string_array_from_value(caller, env_pairs)?;
    let mut out = Vec::with_capacity(pairs.len());
    for pair in pairs {
        if let Some((key, value)) = pair.split_once('=') {
            out.push((key.to_string(), value.to_string()));
        }
    }
    Ok(out)
}

fn run_command(
    caller: &mut Caller<'_, RuntimeState>,
    command: &str,
    args: &[String],
    options: &CommandOptions,
) -> Result<CommandOutcome, String> {
    validate_command_allowed(caller, command, options.shell)?;
    let mut cmd = if options.shell {
        let mut shell = Command::new("sh");
        shell.arg("-c").arg(command);
        shell
    } else {
        let mut direct = Command::new(command);
        direct.args(args);
        direct
    };
    if let Some(cwd) = options.cwd.as_deref().or(caller.data().process.cwd.as_deref()) {
        cmd.current_dir(cwd);
    }
    if !options.env_pairs.is_empty() {
        cmd.env_clear();
        for (key, value) in caller.data().process.env.iter() {
            cmd.env(key, value);
        }
        for (key, value) in &options.env_pairs {
            cmd.env(key, value);
        }
    }
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|err| err.to_string())?;
    if !options.input.is_empty() {
        match child.stdin.as_mut() {
            Some(stdin) => stdin.write_all(&options.input).map_err(|err| err.to_string())?,
            None => return Err("failed to open child stdin".to_string()),
        }
    }
    drop(child.stdin.take());

    let output = if let Some(timeout_ms) = options.timeout_ms {
        match child
            .wait_timeout(Duration::from_millis(timeout_ms))
            .map_err(|err| err.to_string())?
        {
            Some(_) => child.wait_with_output().map_err(|err| err.to_string())?,
            None => {
                let _ = child.kill();
                let output = child.wait_with_output().map_err(|err| err.to_string())?;
                return Err(format!(
                    "Command timed out after {timeout_ms}ms; stdout={} bytes stderr={} bytes",
                    output.stdout.len(),
                    output.stderr.len()
                ));
            }
        }
    } else {
        child.wait_with_output().map_err(|err| err.to_string())?
    };
    let total = output.stdout.len().saturating_add(output.stderr.len());
    if total > options.max_buffer {
        return Err(format!(
            "maxBuffer exceeded: {total} bytes captured, limit is {} bytes",
            options.max_buffer
        ));
    }
    Ok(CommandOutcome {
        stdout: output.stdout,
        stderr: output.stderr,
        status: output.status.code(),
        signal: signal_from_status(&output.status),
    })
}

fn validate_command_allowed(
    caller: &mut Caller<'_, RuntimeState>,
    command: &str,
    shell: bool,
) -> Result<(), String> {
    let raw = caller
        .data()
        .process
        .env
        .iter()
        .rev()
        .find_map(|(key, value)| (key == "WJSM_CHILD_PROCESS_ALLOW").then_some(value.as_str()))
        .unwrap_or("");
    let allowlist = parse_allowlist(raw);
    if allowlist.iter().any(|item| item == "*") {
        return Ok(());
    }
    let command_name = if shell {
        command.split_whitespace().next().unwrap_or(command)
    } else {
        command
    };
    for allowed in &allowlist {
        if allowed == command_name {
            return Ok(());
        }
        let allowed_path = Path::new(allowed);
        let command_path = Path::new(command_name);
        if command_path.components().count() > 1 && paths_match(command_path, allowed_path) {
            return Ok(());
        }
        if allowed_path.components().count() == 1
            && command_path
                .file_name()
                .is_some_and(|file_name| file_name == allowed_path.as_os_str())
        {
            return Ok(());
        }
    }
    Err(format!(
        "child_process execution is disabled for '{command_name}'; set WJSM_CHILD_PROCESS_ALLOW to an allowlisted command or '*'"
    ))
}

fn parse_allowlist(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for segment in raw.split(',') {
        for path in std::env::split_paths(segment) {
            let text = path.to_string_lossy().trim().to_string();
            if !text.is_empty() {
                out.push(text);
            }
        }
    }
    out
}

fn paths_match(command: &Path, allowed: &Path) -> bool {
    canonicalize(command) == canonicalize(allowed)
}

fn canonicalize(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(unix)]
fn signal_from_status(status: &std::process::ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;
    status.signal().map(|signal| format!("SIG{signal}"))
}

#[cfg(not(unix))]
fn signal_from_status(_status: &std::process::ExitStatus) -> Option<String> {
    None
}

fn create_spawn_result(caller: &mut Caller<'_, RuntimeState>, outcome: CommandOutcome) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 6);
    let stdout = create_buffer_from_bytes(caller, outcome.stdout);
    let stderr = create_buffer_from_bytes(caller, outcome.stderr);
    let temp_root_len = caller.data().push_host_temp_roots([obj, stdout, stderr]);
    let _ = define_host_data_property_from_caller(caller, obj, "stdout", stdout);
    let _ = define_host_data_property_from_caller(caller, obj, "stderr", stderr);
    let status = outcome
        .status
        .map(|status| value::encode_f64(status as f64))
        .unwrap_or_else(value::encode_null);
    let signal = outcome
        .signal
        .map(|signal| store_runtime_string(caller, signal))
        .unwrap_or_else(value::encode_null);
    let _ = define_host_data_property_from_caller(caller, obj, "status", status);
    let _ = define_host_data_property_from_caller(caller, obj, "signal", signal);
    let _ = define_host_data_property_from_caller(caller, obj, "pid", value::encode_f64(0.0));
    caller.data().truncate_host_temp_roots(temp_root_len);
    obj
}

fn make_child_process_error(caller: &mut Caller<'_, RuntimeState>, msg: &str) -> i64 {
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
