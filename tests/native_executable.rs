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

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn build_native_executable(args: &[&str], output: &std::path::Path) {
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

fn relocate_and_hide_sources(
    packed: &std::path::Path,
    project: &std::path::Path,
    run_dir: &std::path::Path,
) -> PathBuf {
    std::fs::create_dir_all(run_dir).expect("run dir");
    let relocated = run_dir.join(packed.file_name().expect("exe name"));
    std::fs::copy(packed, &relocated).expect("copy exe");
    let _ = std::fs::remove_file(packed);
    std::fs::remove_dir_all(project).expect("hide source tree");
    relocated
}

fn run_relocated(exe: &std::path::Path) -> std::process::Output {
    Command::new(exe)
        .current_dir(exe.parent().expect("run dir"))
        .output()
        .expect("packed executable should spawn")
}

#[test]
fn native_executable_keeps_virtual_identity_after_relocate() {
    let dir = scratch_dir();
    let project = dir.join("project");
    let out_dir = dir.join("out");
    let run_dir = dir.join("run");
    std::fs::create_dir_all(&project).expect("project dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    let entry = project.join("main.js");
    let dep = project.join("dep.js");
    std::fs::write(
        &entry,
        "import './dep.js';\nconsole.log(import.meta.resolve('./dep.js'));\n",
    )
    .expect("entry");
    std::fs::write(&dep, "export const value = 1;\n").expect("dep");
    let output = out_dir.join(exe_name("app"));
    build_native_executable(&[entry.to_str().expect("utf8")], &output);

    let relocated = relocate_and_hide_sources(&output, &project, &run_dir);
    let run = run_relocated(&relocated);
    assert!(
        run.status.success(),
        "packed executable failed: status={} stdout={} stderr={}",
        run.status,
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout).replace('\\', "/");
    assert_eq!(stdout.trim(), "file:///wjsm-exec/dep.js");
    assert!(
        !stdout.contains(&dir.to_string_lossy().replace('\\', "/")),
        "resolve must not echo the build-machine path, got {stdout}"
    );
}

#[test]
fn native_executable_loads_snapshot_json_and_dynamic_import() {
    let dir = scratch_dir();
    let project = dir.join("project");
    let out_dir = dir.join("out");
    let run_dir = dir.join("run");
    std::fs::create_dir_all(&project).expect("project dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    std::fs::write(
        project.join("main.js"),
        r#"
const spec = './' + 'dyn.js';
const jsonName = './' + 'data.json';
const data = require(jsonName);
const mod = require(spec);
console.log(data.ok, mod.value);
"#,
    )
    .expect("entry");
    std::fs::write(project.join("dyn.js"), "exports.value = 7;\n").expect("dyn");
    std::fs::write(project.join("data.json"), "{\"ok\":1}\n").expect("json");
    let output = out_dir.join(exe_name("snap"));
    build_native_executable(
        &[
            "--include",
            project.join("dyn.js").to_str().expect("utf8"),
            "--include",
            project.join("data.json").to_str().expect("utf8"),
            project.join("main.js").to_str().expect("utf8"),
        ],
        &output,
    );

    let relocated = relocate_and_hide_sources(&output, &project, &run_dir);
    let run = run_relocated(&relocated);
    assert!(
        run.status.success(),
        "packed snapshot load failed: status={} stdout={} stderr={}",
        run.status,
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "1 7");
}

#[test]
fn native_executable_include_worker_survives_relocate() {
    let dir = scratch_dir();
    let project = dir.join("project");
    let out_dir = dir.join("out");
    let run_dir = dir.join("run");
    std::fs::create_dir_all(&project).expect("project dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    std::fs::write(
        project.join("main.js"),
        r#"
const { Worker } = require('worker_threads');
const path = require('path');
const w = new Worker(path.join(__dirname, 'worker.js'));
w.on('message', (m) => {
  console.log(m);
  w.terminate();
});
w.on('error', (err) => {
  console.error(err);
  process.exit(1);
});
w.on('exit', () => process.exit(0));
"#,
    )
    .expect("entry");
    std::fs::write(
        project.join("worker.js"),
        "const { parentPort } = require('worker_threads');\nparentPort.postMessage('from-worker');\n",
    )
    .expect("worker");
    let output = out_dir.join(exe_name("worker"));
    build_native_executable(
        &[
            "--include",
            project.join("worker.js").to_str().expect("utf8"),
            project.join("main.js").to_str().expect("utf8"),
        ],
        &output,
    );

    let relocated = relocate_and_hide_sources(&output, &project, &run_dir);
    let run = run_relocated(&relocated);
    assert!(
        run.status.success(),
        "packed worker failed: status={} stdout={} stderr={}",
        run.status,
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        String::from_utf8_lossy(&run.stdout).contains("from-worker"),
        "worker stdout missing: {}",
        String::from_utf8_lossy(&run.stdout)
    );
}

#[test]
fn native_executable_worker_without_include_fails() {
    let dir = scratch_dir();
    let project = dir.join("project");
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&project).expect("project dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    std::fs::write(
        project.join("main.js"),
        r#"
const { Worker } = require('worker_threads');
const path = require('path');
const w = new Worker(path.join(__dirname, 'worker.js'));
w.on('error', (err) => {
  console.log(String(err));
  process.exit(0);
});
w.on('exit', (code) => process.exit(code === 0 ? 1 : 0));
"#,
    )
    .expect("entry");
    std::fs::write(
        project.join("worker.js"),
        "console.log('should-not-run');\n",
    )
    .expect("worker");
    let output = out_dir.join(exe_name("missing-worker"));
    build_native_executable(&[project.join("main.js").to_str().expect("utf8")], &output);
    let run = Command::new(&output)
        .current_dir(&out_dir)
        .output()
        .expect("packed executable should spawn");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        combined.contains("not in the module source store") || !run.status.success(),
        "missing worker include should fail closed, got status={} output={combined}",
        run.status
    );
    assert!(
        !combined.contains("should-not-run"),
        "worker file outside the snapshot must not execute"
    );
}

#[test]
fn native_executable_missing_import_is_fail_closed() {
    let dir = scratch_dir();
    let project = dir.join("project");
    let out_dir = dir.join("out");
    let run_dir = dir.join("run");
    std::fs::create_dir_all(&project).expect("project dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    std::fs::write(
        project.join("main.js"),
        "const spec = './' + 'missing.js';\ntry { require(spec); } catch (err) { console.log(String(err)); }\n",
    )
    .expect("entry");
    let output = out_dir.join(exe_name("closed"));
    build_native_executable(&[project.join("main.js").to_str().expect("utf8")], &output);

    let relocated = relocate_and_hide_sources(&output, &project, &run_dir);
    std::fs::write(run_dir.join("missing.js"), "console.log('LEAK');\n").expect("decoy");
    let run = run_relocated(&relocated);
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        !stdout.contains("LEAK") && !stderr.contains("LEAK"),
        "packed exe must not read cwd decoy: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("Cannot find module")
            || stdout.contains("not found")
            || stdout.contains("failed")
            || !run.status.success(),
        "missing snapshot module should fail closed, got status={} stdout={stdout} stderr={stderr}",
        run.status
    );
}

#[test]
fn wjsm_run_keeps_host_file_url() {
    let dir = scratch_dir();
    let project = dir.join("project");
    std::fs::create_dir_all(&project).expect("project dir");
    let entry = project.join("main.js");
    let dep = project.join("dep.js");
    std::fs::write(
        &entry,
        "export {};\nconsole.log(import.meta.resolve('./dep.js'));\n",
    )
    .expect("entry");
    std::fs::write(&dep, "export const value = 1;\n").expect("dep");
    let wjsm = env!("CARGO_BIN_EXE_wjsm");
    let run = Command::new(wjsm)
        .arg("run")
        .arg(&entry)
        .output()
        .expect("wjsm run should spawn");
    assert!(
        run.status.success(),
        "wjsm run failed: status={} stderr={}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout).replace('\\', "/");
    let dep = dep.canonicalize().expect("dep path");
    let dep_url = dep.to_string_lossy().replace('\\', "/");
    assert!(
        stdout.starts_with("file://") && stdout.contains(&dep_url),
        "wjsm run should keep host file URLs, got {stdout}"
    );
    assert!(
        !stdout.contains("file:///wjsm-exec/"),
        "wjsm run must not use packed virtual identity, got {stdout}"
    );
}
