use anyhow::Result;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;

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

#[cfg(unix)]
#[test]
fn run_accepts_non_utf8_file_path() -> Result<()> {
    let root = unique_temp_dir("wjsm_non_utf8_run");
    fs::create_dir_all(&root)?;
    let entry = root.join(std::ffi::OsString::from_vec(b"entry_\xFF.js".to_vec()));
    fs::write(&entry, "console.log(7);\n")?;

    let output = Command::new(resolve_binary_path())
        .arg("run")
        .arg(&entry)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");

    assert!(output.status.success(), "stderr: {stderr}");
    assert_eq!(stdout, "7\n");
    assert!(stderr.is_empty());
    Ok(())
}

#[cfg(unix)]
#[test]
fn explicit_root_accepts_non_utf8_module_entry_path() -> Result<()> {
    let root = unique_temp_dir("wjsm_non_utf8_root");
    fs::create_dir_all(&root)?;
    let entry = root.join(std::ffi::OsString::from_vec(b"main_\xFF.js".to_vec()));
    fs::write(
        &entry,
        "import { answer } from './dep.js';\nconsole.log(answer);\n",
    )?;
    fs::write(root.join("dep.js"), "export const answer = 42;\n")?;

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

#[cfg(unix)]
fn unique_temp_dir(name: &str) -> PathBuf {
    let suffix = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos()
    );
    std::env::temp_dir().join(name).join(suffix)
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
