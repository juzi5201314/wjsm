//! `wjsm-exec` stub：从自身 ELF/PE overlay 启动 NativeRuntime。
//!
//! 非目标（禁止实现）：
//! - 把 guest `.text` 合进 stub `PT_LOAD`
//! - stub 自解压 / UPX
//! - `libwjsm.so` 或任何旁路共享库
//! - 从 stub 拿掉 Cranelift / 从 overlay 拿掉 `.wjsm`

use std::io::{self, Write};
use std::process::ExitCode;

use wjsm_exec_format::{ExecFormatError, unpack_from_path};
use wjsm_host_native::{
    InspectorConfig, ModuleSourceStore, NativeRuntime, NativeRuntimeConfig, NativeRuntimeError,
    OutputMode, compile_snapshot_entry, images_from_exec_payload,
};

/// stub 与打包后可执行文件共用的进程入口。
pub fn main_entry() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            let _ = writeln!(io::stderr(), "{error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<ExitCode, RunError> {
    let exe = std::env::current_exe()?;
    let payload = unpack_from_path(&exe).map_err(|error| match error {
        ExecFormatError::MissingFooter | ExecFormatError::InvalidMagic => RunError::BareStub,
        other => RunError::Format(other),
    })?;
    let inspector = InspectorConfig::from_environment().map_err(RunError::Inspect)?;
    let debug_codegen = inspector.is_some();
    let packed_entry = std::env::var("WJSM_EXEC_ENTRY")
        .ok()
        .filter(|entry| !entry.is_empty());
    let (artifact, images) = images_from_exec_payload(&payload)?;
    let mut runtime = NativeRuntime::new_with_config_and_inspector(
        NativeRuntimeConfig::from_environment(None)?.with_output_mode(OutputMode::Inherit),
        inspector,
    )?;
    if let Some(url) = runtime.inspector_url() {
        let _ = writeln!(io::stderr(), "Debugger listening on {url}");
    }
    runtime.configure_environment(true, std::iter::empty::<(String, String)>())?;
    runtime.configure_process_arguments(std::env::args().skip(1))?;
    let working_directory = std::env::current_dir().unwrap_or_else(|_| exe.clone());
    let store = ModuleSourceStore::snapshot(payload.files)
        .map_err(|error| NativeRuntimeError::Invariant(error.to_string()))?;
    let execution = if let Some(entry) = packed_entry {
        let artifact = compile_snapshot_entry(&store, &entry, debug_codegen)?;
        runtime.execute_with_store(&artifact, store, &working_directory)?
    } else if debug_codegen {
        let entry = artifact
            .manifest()
            .modules
            .iter()
            .find(|module| module.id == artifact.manifest().entry)
            .map(|module| module.logical_url.as_str())
            .unwrap_or("main.js");
        match compile_snapshot_entry(&store, entry, true) {
            Ok(debug_artifact) => {
                runtime.execute_with_store(&debug_artifact, store, &working_directory)?
            }
            Err(_) => runtime.execute_with_store(&artifact, store, &working_directory)?,
        }
    } else {
        runtime.execute_precompiled(&artifact, &images, store, &working_directory)?
    };
    let code = u8::try_from(execution.exit_code.rem_euclid(256)).unwrap_or(1);
    Ok(ExitCode::from(code))
}

#[derive(Debug)]
enum RunError {
    BareStub,
    Io(std::io::Error),
    Format(ExecFormatError),
    Inspect(String),
    Runtime(NativeRuntimeError),
}

impl From<std::io::Error> for RunError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<NativeRuntimeError> for RunError {
    fn from(error: NativeRuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BareStub => formatter.write_str(
                "wjsm-exec is the native-executable stub; use `wjsm build --format native-executable`",
            ),
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Format(error) => write!(formatter, "{error}"),
            Self::Inspect(error) => write!(formatter, "{error}"),
            Self::Runtime(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for RunError {}
