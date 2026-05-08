use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;

use crate::fixture_runner::FixtureRunner;

#[test]
fn happy() -> Result<()> {
    FixtureRunner::new()?.run_suite("happy")
}

#[test]
fn errors() -> Result<()> {
    FixtureRunner::new()?.run_suite("errors")
}

#[test]
fn modules() -> Result<()> {
    FixtureRunner::new()?.run_suite("modules")
}

#[test]
fn modules_respects_explicit_root_flag() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("modules")
        .join("named_exports");
    let entry = root.join("main.js");

    let output = Command::new(resolve_binary_path())
        .arg("run")
        .arg(&entry)
        .arg("--root")
        .arg(&root)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");

    assert!(output.status.success(), "stderr: {stderr}");
    assert_eq!(stdout, "42\n");
    assert!(stderr.is_empty());
    Ok(())
}

fn resolve_binary_path() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_wjsm")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("debug")
                .join(binary_name())
        })
}

fn binary_name() -> &'static str {
    if cfg!(windows) { "wjsm.exe" } else { "wjsm" }
}
