//! native-executable 集成测试共用的打包与搬迁辅助。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

pub fn scratch_dir() -> PathBuf {
    let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir()
        .join("wjsm-test-cache")
        .join("native-exec")
        .join(format!("{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("scratch dir");
    path
}

pub fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

pub fn build_native_executable(args: &[&str], output: &Path) {
    let wjsm = env!("CARGO_BIN_EXE_wjsm");
    let stub = env!("CARGO_BIN_EXE_wjsm-exec");
    let status = Command::new(wjsm)
        .args(["build", "--format", "native-executable", "-o"])
        .arg(output)
        .args(args)
        .env("WJSM_EXEC_STUB", stub)
        .status()
        .expect("wjsm build should spawn");
    assert!(
        status.success(),
        "wjsm build --format native-executable failed: {status}"
    );
    assert!(output.is_file(), "native executable should be created");
}

pub fn build_wjsm(args: &[&str], output: &Path) {
    let wjsm = env!("CARGO_BIN_EXE_wjsm");
    let status = Command::new(wjsm)
        .args(["build", "-o"])
        .arg(output)
        .args(args)
        .status()
        .expect("wjsm build should spawn");
    assert!(status.success(), "wjsm build .wjsm failed: {status}");
    assert!(output.is_file(), "portable artifact should be created");
}

pub fn relocate_and_hide_sources(packed: &Path, project: &Path, run_dir: &Path) -> PathBuf {
    std::fs::create_dir_all(run_dir).expect("run dir");
    let relocated = run_dir.join(packed.file_name().expect("exe name"));
    std::fs::copy(packed, &relocated).expect("copy exe");
    let _ = std::fs::remove_file(packed);
    std::fs::remove_dir_all(project).expect("hide source tree");
    relocated
}

pub fn run_relocated(exe: &Path) -> std::process::Output {
    Command::new(exe)
        .current_dir(exe.parent().expect("run dir"))
        .output()
        .expect("packed executable should spawn")
}

#[allow(dead_code)]
pub fn run_inspected(exe: &Path) -> std::process::Output {
    Command::new(exe)
        .current_dir(exe.parent().expect("run dir"))
        .env("WJSM_INSPECT", "127.0.0.1:0")
        .output()
        .expect("packed inspect executable should spawn")
}
