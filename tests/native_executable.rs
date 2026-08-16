//! 同宿主 native-executable：打包 stub+overlay 后直接运行。

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

fn scratch_dir() -> PathBuf {
    let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir()
        .join("wjsm-test-cache")
        .join("native-exec")
        .join(format!("{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("scratch dir");
    path
}

#[test]
fn native_executable_prints_one() {
    let dir = scratch_dir();
    let output = dir.join(if cfg!(windows) { "hello.exe" } else { "hello" });
    let wjsm = env!("CARGO_BIN_EXE_wjsm");
    let stub = env!("CARGO_BIN_EXE_wjsm-exec");
    let status = Command::new(wjsm)
        .args([
            "build",
            "-e",
            "console.log(1)",
            "--format",
            "native-executable",
            "-o",
        ])
        .arg(&output)
        .env("WJSM_EXEC_STUB", stub)
        .status()
        .expect("wjsm build should spawn");
    assert!(
        status.success(),
        "wjsm build --format native-executable failed: {status}"
    );
    assert!(output.is_file(), "native executable should be created");

    let run = Command::new(&output)
        .output()
        .expect("packed executable should spawn");
    assert!(
        run.status.success(),
        "packed executable failed: status={} stderr={}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.stdout, b"1\n");
}

#[test]
fn native_executable_runs_hello_fixture() {
    let dir = scratch_dir();
    let output = dir.join(if cfg!(windows) { "hello.exe" } else { "hello" });
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/happy/hello.js");
    let wjsm = env!("CARGO_BIN_EXE_wjsm");
    let stub = env!("CARGO_BIN_EXE_wjsm-exec");
    let status = Command::new(wjsm)
        .args(["build", "--format", "native-executable", "-o"])
        .arg(&output)
        .arg(&fixture)
        .env("WJSM_EXEC_STUB", stub)
        .status()
        .expect("wjsm build should spawn");
    assert!(
        status.success(),
        "wjsm build hello.js --format native-executable failed: {status}"
    );

    let run = Command::new(&output)
        .output()
        .expect("packed executable should spawn");
    assert!(
        run.status.success(),
        "packed hello executable failed: status={} stderr={}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.stdout, b"Hello, World!\n");
}

#[test]
fn native_executable_keeps_compile_module_root() {
    let dir = scratch_dir();
    let project = dir.join("project");
    let out_dir = dir.join("out");
    let run_dir = dir.join("run");
    std::fs::create_dir_all(&project).expect("project dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    std::fs::create_dir_all(&run_dir).expect("run dir");
    let entry = project.join("main.js");
    let dep = project.join("dep.js");
    std::fs::write(
        &entry,
        "export {};\nconsole.log(import.meta.resolve('./dep.js'));\n",
    )
    .expect("entry");
    std::fs::write(&dep, "export const value = 1;\n").expect("dep");
    let output = out_dir.join(if cfg!(windows) { "app.exe" } else { "app" });
    let wjsm = env!("CARGO_BIN_EXE_wjsm");
    let stub = env!("CARGO_BIN_EXE_wjsm-exec");
    let status = Command::new(wjsm)
        .args(["build", "--format", "native-executable", "-o"])
        .arg(&output)
        .arg(&entry)
        .env("WJSM_EXEC_STUB", stub)
        .status()
        .expect("wjsm build should spawn");
    assert!(
        status.success(),
        "wjsm build --format native-executable failed: {status}"
    );

    let run = Command::new(&output)
        .current_dir(&run_dir)
        .output()
        .expect("packed executable should spawn");
    assert!(
        run.status.success(),
        "packed executable failed: status={} stdout={} stderr={}",
        run.status,
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout).replace('\\', "/");
    let dep = dep.canonicalize().expect("dep path");
    let dep_url = dep.to_string_lossy().replace('\\', "/");
    assert!(
        stdout.starts_with("file://") && stdout.contains(&dep_url),
        "runtime resolve should stay under the compile module root {dep_url}, got {stdout}"
    );
}
