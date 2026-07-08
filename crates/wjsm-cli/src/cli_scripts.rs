use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

pub(crate) struct PackageScript {
    root: PathBuf,
    scripts: Map<String, Value>,
}

impl PackageScript {
    pub(crate) fn discover(start: Option<&Path>) -> Result<Option<Self>> {
        let mut current = match start {
            Some(path) if path.is_file() => path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf(),
            Some(path) => path.to_path_buf(),
            None => env::current_dir().context("failed to read current directory")?,
        };
        loop {
            let package_json = current.join("package.json");
            if package_json.is_file() {
                let content = fs::read_to_string(&package_json)
                    .with_context(|| format!("failed to read '{}'", package_json.display()))?;
                let package: Value = serde_json::from_str(&content)
                    .with_context(|| format!("failed to parse '{}'", package_json.display()))?;
                let scripts = package
                    .get("scripts")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                return Ok(Some(Self {
                    root: current,
                    scripts,
                }));
            }
            if !current.pop() {
                return Ok(None);
            }
        }
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.script_command(name).is_some()
    }

    pub(crate) fn run(&self, name: &str, args: &[OsString]) -> Result<()> {
        self.run_lifecycle(&format!("pre{name}"), &[])?;
        self.run_lifecycle(name, args)?;
        self.run_lifecycle(&format!("post{name}"), &[])
    }

    fn run_lifecycle(&self, name: &str, args: &[OsString]) -> Result<()> {
        let Some(command) = self.script_command(name) else {
            return Ok(());
        };
        let mut full_command = command;
        for arg in args {
            full_command.push(' ');
            full_command.push_str(&shell_quote(arg));
        }
        let status = shell_command(&full_command)
            .current_dir(&self.root)
            .env("PATH", script_path(&self.root))
            .status()
            .with_context(|| format!("failed to run package script `{name}`"))?;
        if !status.success() {
            bail!("package script `{name}` failed with status {status}");
        }
        Ok(())
    }

    fn script_command(&self, name: &str) -> Option<String> {
        self.scripts
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

pub(crate) fn run_package_script(
    start: Option<&Path>,
    name: &str,
    args: &[OsString],
) -> Result<()> {
    let Some(package) = PackageScript::discover(start)? else {
        bail!("package.json not found for script `{name}`");
    };
    if !package.contains(name) {
        bail!("package script `{name}` not found");
    }
    package.run(name, args)
}

pub(crate) fn package_script_exists(start: Option<&Path>, name: &str) -> Result<bool> {
    Ok(PackageScript::discover(start)?.is_some_and(|package| package.contains(name)))
}

fn script_path(root: &Path) -> OsString {
    let mut paths = Vec::new();
    paths.push(root.join("node_modules").join(".bin"));
    if let Ok(current_exe) = env::current_exe()
        && let Some(parent) = current_exe.parent()
    {
        paths.push(parent.to_path_buf());
    }
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    env::join_paths(paths).unwrap_or_else(|_| env::var_os("PATH").unwrap_or_default())
}

#[cfg(unix)]
fn shell_command(command: &str) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut cmd = Command::new("cmd");
    cmd.arg("/C").arg(command);
    cmd
}

fn shell_quote(value: &OsStr) -> String {
    let raw = value.to_string_lossy();
    if raw
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':' | '='))
    {
        return raw.into_owned();
    }
    let escaped = raw.replace('\'', "'\\''");
    format!("'{escaped}'")
}
