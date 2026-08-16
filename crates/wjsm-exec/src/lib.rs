//! `wjsm-exec` stub：从自身 ELF/PE overlay 启动 NativeRuntime。

use std::io::{self, Write};
use std::process::ExitCode;

use wjsm_exec_format::{ExecFormatError, unpack};
use wjsm_host_native::{
    NativeRuntime, NativeRuntimeConfig, NativeRuntimeError, images_from_exec_payload,
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
    let bytes = std::fs::read(&exe)?;
    let payload = unpack(&bytes).map_err(|error| match error {
        ExecFormatError::MissingFooter | ExecFormatError::InvalidMagic => RunError::BareStub,
        other => RunError::Format(other),
    })?;
    let (artifact, images) = images_from_exec_payload(&payload)?;
    let mut runtime = NativeRuntime::new_with_config(
        NativeRuntimeConfig::from_environment(None)?.with_specialization_enabled(false),
    )?;
    runtime.configure_environment(true, std::iter::empty::<(String, String)>())?;
    runtime.configure_process_arguments(std::env::args())?;
    let working_directory = std::env::current_dir().unwrap_or_else(|_| exe.clone());
    let module_root = std::path::Path::new(&payload.module_root);
    let execution =
        runtime.execute_precompiled(&artifact, &images, module_root, &working_directory)?;
    io::stdout().write_all(&execution.stdout)?;
    io::stderr().write_all(&execution.stderr)?;
    let code = u8::try_from(execution.exit_code.rem_euclid(256)).unwrap_or(1);
    Ok(ExitCode::from(code))
}

#[derive(Debug)]
enum RunError {
    BareStub,
    Io(std::io::Error),
    Format(ExecFormatError),
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
            Self::Runtime(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for RunError {}
