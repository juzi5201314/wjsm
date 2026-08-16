//! ADR 0016 / 0017 / 0019 里尚未被主文件覆盖的 packed 合同。

mod native_exec;

use std::io::Write;
use std::process::{Command, Stdio};

use native_exec::{
    build_native_executable, build_wjsm, exe_name, relocate_and_hide_sources, run_inspected,
    run_relocated, scratch_dir,
};

#[test]
fn native_executable_inspect_relowers_from_snapshot() {
    let dir = scratch_dir();
    let project = dir.join("project");
    let out_dir = dir.join("out");
    let run_dir = dir.join("run");
    std::fs::create_dir_all(&project).expect("project dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    std::fs::write(
        project.join("main.js"),
        "import './dep.js';\nconsole.log(import.meta.resolve('./dep.js'));\n",
    )
    .expect("entry");
    std::fs::write(project.join("dep.js"), "export const value = 1;\n").expect("dep");
    let output = out_dir.join(exe_name("inspect"));
    build_native_executable(&[project.join("main.js").to_str().expect("utf8")], &output);
    let relocated = relocate_and_hide_sources(&output, &project, &run_dir);
    let baseline = run_relocated(&relocated);
    let inspected = run_inspected(&relocated);
    assert!(
        baseline.status.success(),
        "precompiled failed: {}",
        String::from_utf8_lossy(&baseline.stderr)
    );
    assert!(
        inspected.status.success(),
        "inspect relower failed: stdout={} stderr={}",
        String::from_utf8_lossy(&inspected.stdout),
        String::from_utf8_lossy(&inspected.stderr)
    );
    let stderr = String::from_utf8_lossy(&inspected.stderr);
    assert!(
        stderr.contains("Debugger listening on"),
        "inspect should announce CDP: {stderr}"
    );
    assert_eq!(inspected.stdout, baseline.stdout);
    let stdout = String::from_utf8_lossy(&inspected.stdout).replace('\\', "/");
    assert_eq!(stdout.trim(), "file:///wjsm-exec/dep.js");
}

#[test]
fn native_executable_eval_inspect_uses_snapshot() {
    let dir = scratch_dir();
    let output = dir.join(exe_name("eval-inspect"));
    build_native_executable(&["-e", "console.log(1)"], &output);
    let inspected = run_inspected(&output);
    assert!(
        inspected.status.success(),
        "eval inspect failed: stdout={} stderr={}",
        String::from_utf8_lossy(&inspected.stdout),
        String::from_utf8_lossy(&inspected.stderr)
    );
    assert_eq!(inspected.stdout, b"1\n");
    let stderr = String::from_utf8_lossy(&inspected.stderr);
    assert!(
        stderr.contains("Debugger listening on"),
        "eval inspect should announce CDP: {stderr}"
    );
}

#[test]
fn native_executable_inspect_requires_snapshot_entry() {
    let dir = scratch_dir();
    let output = dir.join(exe_name("missing-eval"));
    build_native_executable(&["-e", "console.log(1)"], &output);
    let packed = std::fs::read(&output).expect("read packed");
    let mut payload = wjsm_exec_format::unpack(&packed).expect("unpack");
    payload.files.remove("eval.js");
    let tampered = wjsm_exec_format::pack(&packed, &payload).expect("repack");
    std::fs::write(&output, tampered).expect("write tampered");
    let baseline = Command::new(&output).output().expect("run precompiled");
    assert!(
        baseline.status.success(),
        "precompiled should still run: {}",
        String::from_utf8_lossy(&baseline.stderr)
    );
    assert_eq!(baseline.stdout, b"1\n");
    let inspected = run_inspected(&output);
    assert!(
        !inspected.status.success(),
        "inspect without snapshot entry must fail: stdout={} stderr={}",
        String::from_utf8_lossy(&inspected.stdout),
        String::from_utf8_lossy(&inspected.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&inspected.stdout),
        String::from_utf8_lossy(&inspected.stderr)
    );
    assert!(
        combined.contains("not in the module source store") || combined.contains("eval.js"),
        "inspect miss should mention the snapshot entry: {combined}"
    );
}

#[test]
fn native_executable_from_wjsm_include_json_and_dynamic_import() {
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
    let artifact = out_dir.join("app.wjsm");
    build_wjsm(
        &[
            "--root",
            project.to_str().expect("utf8"),
            project.join("main.js").to_str().expect("utf8"),
        ],
        &artifact,
    );
    let output = out_dir.join(exe_name("wjsm-snap"));
    build_native_executable(
        &[
            "--root",
            project.to_str().expect("utf8"),
            "--include",
            project.join("dyn.js").to_str().expect("utf8"),
            "--include",
            project.join("data.json").to_str().expect("utf8"),
            artifact.to_str().expect("utf8"),
        ],
        &output,
    );
    let relocated = relocate_and_hide_sources(&output, &project, &run_dir);
    let run = run_relocated(&relocated);
    assert!(
        run.status.success(),
        "packed .wjsm snapshot load failed: status={} stdout={} stderr={}",
        run.status,
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "1 7");
}

#[test]
fn native_executable_packs_package_exports() {
    let dir = scratch_dir();
    let project = dir.join("project");
    let out_dir = dir.join("out");
    let run_dir = dir.join("run");
    std::fs::create_dir_all(project.join("node_modules/pkg")).expect("pkg dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    std::fs::write(
        project.join("main.js"),
        "import { value } from 'pkg';\nconsole.log(value);\n",
    )
    .expect("entry");
    std::fs::write(
        project.join("node_modules/pkg/package.json"),
        r#"{"type":"module","exports":{".":"./lib.js"}}"#,
    )
    .expect("pkg json");
    std::fs::write(
        project.join("node_modules/pkg/lib.js"),
        "export const value = 42;\n",
    )
    .expect("pkg lib");
    let output = out_dir.join(exe_name("pkg"));
    build_native_executable(
        &[
            "--root",
            project.to_str().expect("utf8"),
            project.join("main.js").to_str().expect("utf8"),
        ],
        &output,
    );
    let relocated = relocate_and_hide_sources(&output, &project, &run_dir);
    std::fs::write(run_dir.join("package.json"), "{\"ok\":\"LEAK\"}\n").expect("decoy");
    let run = run_relocated(&relocated);
    assert!(
        run.status.success(),
        "packed package exports failed: stdout={} stderr={}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "42");
}

#[cfg(unix)]
#[test]
fn native_executable_stdin_is_live() {
    let dir = scratch_dir();
    let project = dir.join("project");
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&project).expect("project dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    std::fs::write(
        project.join("main.js"),
        "const fs = require('fs');\nconsole.log('ready');\nconst text = fs.readFileSync('/dev/stdin', 'utf8');\nconsole.log(text.trim());\n",
    )
    .expect("entry");
    let output = out_dir.join(exe_name("stdin"));
    build_native_executable(&[project.join("main.js").to_str().expect("utf8")], &output);
    let mut child = Command::new(&output)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stdin exe");
    let stdout = child.stdout.take().expect("stdout");
    let mut stdin = child.stdin.take().expect("stdin");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        let mut ready = String::new();
        let _ = std::io::BufRead::read_line(&mut reader, &mut ready);
        let _ = tx.send(ready);
        let mut rest = String::new();
        let _ = std::io::Read::read_to_string(&mut reader, &mut rest);
        let _ = tx.send(rest);
    });
    let ready = rx
        .recv_timeout(std::time::Duration::from_secs(8))
        .expect("packed exe should emit stdout before reading stdin");
    assert!(ready.contains("ready"), "live stdout missing: {ready:?}");
    stdin.write_all(b"pong\n").expect("write stdin");
    drop(stdin);
    let rest = rx
        .recv_timeout(std::time::Duration::from_secs(8))
        .expect("packed exe should echo stdin");
    let status = child.wait().expect("wait stdin exe");
    assert!(status.success(), "stdin exe failed: rest={rest:?}");
    assert!(rest.contains("pong"), "stdin echo missing: {rest:?}");
}
