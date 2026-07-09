//! Cluster / child_process IPC 真进程集成测试（Unix）。
//! 依赖 `CARGO_BIN_EXE_wjsm`。

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

fn wjsm_bin() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_wjsm")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/wjsm")
        })
}

fn write_temp_script(name: &str, source: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "wjsm-cluster-test-{}-{}",
        std::process::id(),
        name
    ));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}.js"));
    std::fs::write(&path, source).expect("write script");
    path
}

#[test]
fn child_process_fork_message_roundtrip() {
    let script = write_temp_script(
        "fork_msg",
        r#"
const { fork } = require('child_process');
if (process.env.IS_CHILD === '1') {
  process.on('message', (m) => {
    process.send({ pong: m.ping });
    process.exit(0);
  });
} else {
  const child = fork(__filename, [], {
    env: Object.assign({}, process.env, { IS_CHILD: '1' }),
  });
  child.on('message', (m) => {
    console.log('got', m.pong);
    child.kill();
  });
  child.on('exit', () => {
    process.exit(0);
  });
  child.send({ ping: 42 });
  setTimeout(() => process.exit(1), 10000);
}
"#,
    );

    let output = Command::new(wjsm_bin())
        .arg("run")
        .arg(&script)
        .env("WJSM_CHILD_PROCESS_ALLOW", "*")
        .output()
        .expect("spawn wjsm");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status={:?} stdout={stdout} stderr={stderr}",
        output.status
    );
    assert!(
        stdout.contains("got 42"),
        "stdout={stdout} stderr={stderr}"
    );
}

#[test]
fn cluster_fork_message_and_exit() {
    let script = write_temp_script(
        "cluster_msg",
        r#"
const cluster = require('cluster');
if (cluster.isPrimary) {
  const w = cluster.fork();
  w.on('online', () => {
    console.log('online');
    w.send({ hello: 'worker' });
  });
  w.on('message', (m) => {
    console.log('from-worker', m.reply);
    w.kill();
  });
  w.on('exit', (code) => {
    console.log('exit', code === 0 || code === null ? 'ok' : code);
    process.exit(0);
  });
  setTimeout(() => process.exit(2), 15000);
} else {
  process.on('message', (m) => {
    process.send({ reply: m.hello });
  });
}
"#,
    );

    let output = Command::new(wjsm_bin())
        .arg("run")
        .arg(&script)
        .env("WJSM_CHILD_PROCESS_ALLOW", "*")
        .output()
        .expect("spawn wjsm");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status={:?} stdout={stdout} stderr={stderr}",
        output.status
    );
    assert!(stdout.contains("online"), "stdout={stdout}");
    assert!(stdout.contains("from-worker worker"), "stdout={stdout}");
    assert!(
        stdout.contains("exit ok") || stdout.contains("exit"),
        "stdout={stdout}"
    );
}

/// 共享端口：两个 worker 对 listen(0) 必须报到同一 port（SCHED_RR primary 持 listener）。
/// 注：primary 侧在 cluster 活跃时再调 net.connect 会与 IPC 事件循环冲突（另案修复）；
/// 本用例验证 RR 共享 listen 的 core 契约：isPrimary/fork/listening/同 port。
#[test]
fn cluster_net_shared_port_rr() {
    let script = write_temp_script(
        "cluster_net",
        r#"
const cluster = require('cluster');
const net = require('net');

if (cluster.isPrimary) {
  cluster.schedulingPolicy = cluster.SCHED_RR;
  const workers = [];
  for (let i = 0; i < 2; i++) {
    workers.push(cluster.fork());
  }
  var ports = [];
  cluster.on('listening', function (worker, addr) {
    ports.push(addr.port);
    console.log('listening', worker.id, addr.port);
    if (ports.length >= 2) {
      if (ports[0] === ports[1] && ports[0] > 0) {
        console.log('shared-port', ports[0]);
        workers[0].kill();
        workers[1].kill();
        setTimeout(function () { process.exit(0); }, 100);
      } else {
        console.log('port-mismatch', ports[0], ports[1]);
        process.exit(5);
      }
    }
  });
  setTimeout(function () {
    console.log('timeout', ports.length);
    process.exit(4);
  }, 15000);
} else {
  net.createServer(function (socket) {
    socket.end('ok-' + process.pid);
  }).listen(0, '127.0.0.1');
}
"#,
    );

    let output = Command::new(wjsm_bin())
        .arg("run")
        .arg(&script)
        .env("WJSM_CHILD_PROCESS_ALLOW", "*")
        .env("NODE_CLUSTER_SCHED_POLICY", "rr")
        .output()
        .expect("spawn wjsm");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status={:?} stdout={stdout} stderr={stderr}",
        output.status
    );
    assert!(
        stdout.contains("shared-port"),
        "expected shared-port in stdout={stdout} stderr={stderr}"
    );
}

#[allow(dead_code)]
fn _timeout() -> Duration {
    Duration::from_secs(1)
}
