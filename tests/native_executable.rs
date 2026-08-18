//! 同宿主 native-executable：打包 stub+overlay 后直接运行。

mod native_exec;

use std::path::{Path, PathBuf};
use std::process::Command;

use native_exec::{
    build_native_executable, build_wjsm, exe_name, relocate_and_hide_sources, run_relocated,
    scratch_dir,
};

fn assert_packed_file_contains(path: &Path, expected: &[u8], label: &str) {
    let packed = std::fs::read(path).expect("read packed executable");
    let payload = wjsm_exec_format::unpack(&packed).expect("unpack packed executable");
    assert!(
        payload
            .files
            .values()
            .any(|content| content.as_slice() == expected),
        "{label} source should be embedded in packed files"
    );
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
fn native_executable_string_normalize_uses_stub_icu() {
    let dir = scratch_dir();
    let output = dir.join(if cfg!(windows) {
        "normalize.exe"
    } else {
        "normalize"
    });
    let wjsm = env!("CARGO_BIN_EXE_wjsm");
    let stub = env!("CARGO_BIN_EXE_wjsm-exec");
    let status = Command::new(wjsm)
        .args([
            "build",
            "-e",
            r#"var decomposed = "e\u0301"; var composed = "\u00e9"; console.log(decomposed.normalize() === composed.normalize()); console.log("\ufb00".normalize("NFKC") === "ff");"#,
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
        "wjsm build --format native-executable normalize failed: {status}"
    );

    let run = Command::new(&output)
        .output()
        .expect("packed normalize executable should spawn");
    assert!(
        run.status.success(),
        "packed normalize executable failed: status={} stderr={}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.stdout, b"true\ntrue\n");
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
fn native_executable_from_wjsm_keeps_virtual_identity_after_relocate() {
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
    let artifact = out_dir.join("app.wjsm");
    build_wjsm(
        &[
            "--root",
            project.to_str().expect("utf8"),
            entry.to_str().expect("utf8"),
        ],
        &artifact,
    );
    let output = out_dir.join(exe_name("app"));
    build_native_executable(
        &[
            "--root",
            project.to_str().expect("utf8"),
            artifact.to_str().expect("utf8"),
        ],
        &output,
    );

    let relocated = relocate_and_hide_sources(&output, &project, &run_dir);
    let run = run_relocated(&relocated);
    assert!(
        run.status.success(),
        "packed .wjsm executable failed: status={} stdout={} stderr={}",
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
fn native_executable_from_wjsm_without_sources_fails() {
    let dir = scratch_dir();
    let project = dir.join("project");
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&project).expect("project dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    let entry = project.join("main.js");
    std::fs::write(&entry, "console.log(1);\n").expect("entry");
    let artifact = out_dir.join("app.wjsm");
    build_wjsm(&[entry.to_str().expect("utf8")], &artifact);
    let output = out_dir.join(exe_name("missing"));
    let wjsm = env!("CARGO_BIN_EXE_wjsm");
    let stub = env!("CARGO_BIN_EXE_wjsm-exec");
    let result = Command::new(wjsm)
        .args(["build", "--format", "native-executable", "-o"])
        .arg(&output)
        .arg(&artifact)
        .env("WJSM_EXEC_STUB", stub)
        .output()
        .expect("wjsm build should spawn");
    assert!(
        !result.status.success(),
        "packing .wjsm without sources must fail"
    );
    assert!(
        !output.exists(),
        "failed pack must not write the executable"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("failed to include artifact module") || stderr.contains("pass --root"),
        "missing source should mention the artifact module, got {stderr}"
    );
}

#[test]
fn native_executable_from_wjsm_auto_includes_static_worker() {
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
const w = new Worker('./worker.js');
w.on('message', (m) => {
  console.log(m);
  w.terminate();
});
w.on('exit', () => process.exit(0));
"#,
    )
    .expect("entry");
    let worker_source =
        b"const { parentPort } = require('worker_threads');\nparentPort.postMessage('from-wjsm');\n";
    std::fs::write(project.join("worker.js"), worker_source).expect("worker");
    let artifact = out_dir.join("app.wjsm");
    build_wjsm(
        &[
            "--root",
            project.to_str().expect("utf8"),
            project.join("main.js").to_str().expect("utf8"),
        ],
        &artifact,
    );
    let output = out_dir.join(exe_name("wjsm-worker"));
    build_native_executable(
        &[
            "--root",
            project.to_str().expect("utf8"),
            artifact.to_str().expect("utf8"),
        ],
        &output,
    );
    let relocated = relocate_and_hide_sources(&output, &project, &run_dir);
    assert_packed_file_contains(&relocated, worker_source, "static worker");
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
    let worker_source =
        b"const { parentPort } = require('worker_threads');\nparentPort.postMessage('from-worker');\n";
    std::fs::write(project.join("worker.js"), worker_source).expect("worker");
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
    assert_packed_file_contains(&relocated, worker_source, "explicit worker");
}

#[test]
fn native_executable_dynamic_worker_requires_include() {
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
    let worker_source = b"console.log('should-not-run');\n";
    std::fs::write(project.join("worker.js"), worker_source).expect("worker");
    let output = out_dir.join(exe_name("missing-worker"));
    build_native_executable(&[project.join("main.js").to_str().expect("utf8")], &output);

    let packed = std::fs::read(&output).expect("read packed executable");
    let payload = wjsm_exec_format::unpack(&packed).expect("unpack packed executable");
    assert!(
        payload
            .files
            .values()
            .all(|content| content.as_slice() != worker_source),
        "dynamic worker source must require an explicit --include"
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

#[test]
fn native_executable_process_argv_is_node_shaped() {
    let dir = scratch_dir();
    let project = dir.join("project");
    let out_dir = dir.join("out");
    let run_dir = dir.join("run");
    std::fs::create_dir_all(&project).expect("project dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    std::fs::write(
        project.join("main.js"),
        r#"
console.log(process.argv[1]);
console.log(process.__wjsm_packed);
console.log(process.argv.slice(2).join(','));
"#,
    )
    .expect("entry");
    let output = out_dir.join(exe_name("argv"));
    build_native_executable(&[project.join("main.js").to_str().expect("utf8")], &output);
    let relocated = relocate_and_hide_sources(&output, &project, &run_dir);
    let run = Command::new(&relocated)
        .current_dir(&run_dir)
        .args(["foo", "bar"])
        .output()
        .expect("run");
    assert!(
        run.status.success(),
        "argv exe failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines.first().copied(), Some("/wjsm-exec/main.js"));
    assert_eq!(lines.get(1).copied(), Some("true"));
    assert_eq!(lines.get(2).copied(), Some("foo,bar"));
    assert!(
        !stdout.contains(&dir.to_string_lossy().replace('\\', "/")),
        "argv must not echo build path: {stdout}"
    );
}

#[test]
fn native_executable_reads_snapshot_fs_from_dirname() {
    let dir = scratch_dir();
    let project = dir.join("project");
    let out_dir = dir.join("out");
    let run_dir = dir.join("run");
    std::fs::create_dir_all(&project).expect("project dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    std::fs::write(
        project.join("main.js"),
        r#"
const fs = require('fs');
const path = require('path');
const text = fs.readFileSync(path.join(__dirname, 'data.json'), 'utf8');
console.log(JSON.parse(text).ok);
try {
  fs.writeFileSync(path.join(__dirname, 'nope.txt'), 'x');
  console.log('WROTE');
} catch (err) {
  console.log(err.code || String(err));
}
"#,
    )
    .expect("entry");
    std::fs::write(project.join("data.json"), "{\"ok\":9}\n").expect("json");
    let output = out_dir.join(exe_name("fs"));
    build_native_executable(
        &[
            "--include",
            project.join("data.json").to_str().expect("utf8"),
            project.join("main.js").to_str().expect("utf8"),
        ],
        &output,
    );
    let relocated = relocate_and_hide_sources(&output, &project, &run_dir);
    std::fs::write(run_dir.join("data.json"), "{\"ok\":\"LEAK\"}\n").expect("decoy");
    let run = run_relocated(&relocated);
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "fs exe failed: {stdout} {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(stdout.contains("9"), "snapshot read missing: {stdout}");
    assert!(
        stdout.contains("EROFS") || stdout.contains("EACCES"),
        "virtual write should fail: {stdout}"
    );
    assert!(!stdout.contains("LEAK") && !stdout.contains("WROTE"));
}

#[test]
fn native_executable_auto_includes_static_worker() {
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
const w = new Worker('./worker.js');
w.on('message', (m) => {
  console.log(m);
  w.terminate();
});
w.on('exit', () => process.exit(0));
"#,
    )
    .expect("entry");
    let worker_source =
        b"const { parentPort } = require('worker_threads');\nparentPort.postMessage('auto-worker');\n";
    std::fs::write(project.join("worker.js"), worker_source).expect("worker");
    let output = out_dir.join(exe_name("auto-worker"));
    build_native_executable(&[project.join("main.js").to_str().expect("utf8")], &output);
    let relocated = relocate_and_hide_sources(&output, &project, &run_dir);
    assert_packed_file_contains(&relocated, worker_source, "static worker");
}

#[test]
fn native_executable_forks_same_exe() {
    let dir = scratch_dir();
    let project = dir.join("project");
    let out_dir = dir.join("out");
    let run_dir = dir.join("run");
    std::fs::create_dir_all(&project).expect("project dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    std::fs::write(
        project.join("main.js"),
        r#"
const { fork } = require('child_process');
let got = false;
const child = fork('./child.js');
child.on('message', (m) => {
  console.log(m.msg);
  console.log(m.exe === process.execPath ? 'same-exe' : 'other-exe');
  got = true;
  child.disconnect();
});
child.on('error', (err) => {
  console.error(err);
  process.exit(1);
});
child.on('exit', (code) => {
  if (!got) process.exit(code || 1);
});
"#,
    )
    .expect("entry");
    std::fs::write(
        project.join("child.js"),
        "process.send({ msg: 'from-fork', exe: process.execPath });\n",
    )
    .expect("child");
    let output = out_dir.join(exe_name("fork"));
    build_native_executable(&[project.join("main.js").to_str().expect("utf8")], &output);
    let relocated = relocate_and_hide_sources(&output, &project, &run_dir);
    let run = run_relocated(&relocated);
    assert!(
        run.status.success(),
        "fork failed: status={} stdout={} stderr={}",
        run.status,
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("from-fork"), "fork stdout: {stdout}");
    assert!(
        stdout.contains("same-exe"),
        "fork should re-exec packed exe: {stdout}"
    );
}

#[test]
fn native_executable_stdout_is_live() {
    let dir = scratch_dir();
    let project = dir.join("project");
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&project).expect("project dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    std::fs::write(
        project.join("main.js"),
        "console.log('ready');\nwhile (true) {}\n",
    )
    .expect("entry");
    let output = out_dir.join(exe_name("live"));
    build_native_executable(&[project.join("main.js").to_str().expect("utf8")], &output);
    let mut child = Command::new(&output)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn live exe");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(stdout), &mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(std::time::Duration::from_secs(8))
        .expect("packed exe should emit stdout before exit");
    assert!(line.contains("ready"), "live stdout missing: {line:?}");
    assert!(
        child.try_wait().ok().flatten().is_none(),
        "process should still be running when stdout arrives"
    );
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn native_executable_cluster_forks_same_exe() {
    let dir = scratch_dir();
    let project = dir.join("project");
    let out_dir = dir.join("out");
    let run_dir = dir.join("run");
    std::fs::create_dir_all(&project).expect("project dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    std::fs::write(
        project.join("main.js"),
        r#"
const cluster = require('cluster');
if (cluster.isPrimary) {
  const worker = cluster.fork();
  worker.on('message', (m) => {
    console.log(m.msg);
    console.log(m.exe === process.execPath ? 'same-exe' : 'other-exe');
    worker.kill();
  });
  worker.on('exit', () => process.exit(0));
} else {
  process.send({ msg: 'from-cluster', exe: process.execPath });
}
"#,
    )
    .expect("entry");
    let output = out_dir.join(exe_name("cluster"));
    build_native_executable(&[project.join("main.js").to_str().expect("utf8")], &output);
    let relocated = relocate_and_hide_sources(&output, &project, &run_dir);
    let run = run_relocated(&relocated);
    assert!(
        run.status.success(),
        "cluster failed: status={} stdout={} stderr={}",
        run.status,
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("from-cluster"), "cluster stdout: {stdout}");
    assert!(
        stdout.contains("same-exe"),
        "cluster should re-exec packed exe: {stdout}"
    );
}

#[test]
fn native_executable_can_disable_specialization() {
    let dir = scratch_dir();
    let project = dir.join("project");
    let out_dir = dir.join("out");
    let run_dir = dir.join("run");
    std::fs::create_dir_all(&project).expect("project dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    std::fs::write(project.join("main.js"), "console.log(7);\n").expect("entry");
    let output = out_dir.join(exe_name("nospec"));
    build_native_executable(&[project.join("main.js").to_str().expect("utf8")], &output);
    let relocated = relocate_and_hide_sources(&output, &project, &run_dir);
    let run = Command::new(&relocated)
        .current_dir(&run_dir)
        .env("WJSM_DISABLE_SPECIALIZATION", "1")
        .output()
        .expect("run");
    assert!(
        run.status.success(),
        "disable specialization failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "7\n");
}

#[test]
fn native_executable_rejects_settings_mismatch() {
    let dir = scratch_dir();
    let project = dir.join("project");
    let out_dir = dir.join("out");
    let run_dir = dir.join("run");
    std::fs::create_dir_all(&project).expect("project dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    std::fs::write(project.join("main.js"), "console.log(1);\n").expect("entry");
    let output = out_dir.join(exe_name("settings"));
    build_native_executable(&[project.join("main.js").to_str().expect("utf8")], &output);
    let relocated = relocate_and_hide_sources(&output, &project, &run_dir);
    let packed = std::fs::read(&relocated).expect("read packed");
    let mut payload = wjsm_exec_format::unpack(&packed).expect("unpack");
    payload.settings = "tampered-settings".into();
    let tampered = wjsm_exec_format::pack(&packed, &payload).expect("repack");
    std::fs::write(&relocated, tampered).expect("write tampered");
    let run = run_relocated(&relocated);
    assert!(
        !run.status.success(),
        "tampered settings should fail: stdout={} stderr={}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("settings"),
        "settings mismatch should mention settings: {stderr}"
    );
}
