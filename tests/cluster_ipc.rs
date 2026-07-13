//! Cluster / child_process IPC 真进程集成测试（Unix）。
//! 依赖 `CARGO_BIN_EXE_wjsm`。

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::Once;

static ENV_INIT: Once = Once::new();

fn ensure_cluster_test_env() {
    ENV_INIT.call_once(|| {
        // SAFETY: 测试初始化早期设置一次。
        unsafe {
            if std::env::var_os("WJSM_COMPILER").is_none() {
                std::env::set_var("WJSM_COMPILER", "winch");
            }
            if std::env::var_os("WJSM_CACHE_DIR").is_none() {
                std::env::set_var("WJSM_CACHE_DIR", "/tmp/wjsm-test-cache");
            }
        }
    });
}

fn wjsm_bin() -> PathBuf {
    ensure_cluster_test_env();
    std::env::var("CARGO_BIN_EXE_wjsm")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/wjsm"))
}

fn write_temp_script(name: &str, source: &str) -> PathBuf {
    ensure_cluster_test_env();
    // 稳定路径：内容 hash 决定目录，避免每次 pid 不同导致整包 WASM 重编译（1.3MB Winch ~1.5s）。
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    name.hash(&mut hasher);
    source.hash(&mut hasher);
    let key = format!("{:016x}", hasher.finish());
    let dir = std::env::temp_dir().join("wjsm-cluster-test").join(&key);
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}.js"));
    if !path.exists() {
        std::fs::write(&path, source).expect("write script");
    }
    path
}

/// 统一 spawn 父进程：Winch + pipeline/cwasm cache + child_process allow。
fn run_wjsm_script(script: &std::path::Path, extra_env: &[(&str, &str)]) -> std::process::Output {
    ensure_cluster_test_env();
    let cache = std::env::var("WJSM_CACHE_DIR").unwrap_or_else(|_| "/tmp/wjsm-test-cache".into());
    let compiler = std::env::var("WJSM_COMPILER").unwrap_or_else(|_| "winch".into());
    let mut cmd = Command::new(wjsm_bin());
    cmd.arg("run")
        .arg(script)
        .env("WJSM_CHILD_PROCESS_ALLOW", "*")
        .env("WJSM_COMPILER", &compiler)
        .env("WJSM_CACHE_DIR", &cache);
    for (k, v) in extra_env {
        cmd.env(*k, *v);
    }
    cmd.output().expect("spawn wjsm")
}

#[test]
fn child_process_exit_preserves_async_local_storage() {
    let script = write_temp_script(
        "async_hooks_child_exit",
        r#"
const { AsyncLocalStorage } = require('node:async_hooks');
const { spawn } = require('node:child_process');
const als = new AsyncLocalStorage();

als.enterWith('child-context');
const child = spawn('/bin/sh', ['-c', 'exit 0']);
als.enterWith('changed');
child.on('exit', () => {
  console.log(als.getStore());
  process.exit(0);
});
"#,
    );
    let output = run_wjsm_script(&script, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status={:?} stdout={stdout} stderr={stderr}",
        output.status
    );
    assert_eq!(stdout.trim(), "child-context", "stderr={stderr}");
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
  });
} else {
  const child = fork(__filename, [], {
    env: Object.assign({}, process.env, { IS_CHILD: '1' }),
  });
  const watchdog = setTimeout(() => process.exit(1), 10000);
  child.on('message', (m) => {
    console.log('got', m.pong);
    child.kill();
  });
  child.on('exit', () => {
    clearTimeout(watchdog);
    process.exit(0);
  });
  child.send({ ping: 42 });
}
"#,
    );

    let output = run_wjsm_script(&script, &[]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status={:?} stdout={stdout} stderr={stderr}",
        output.status
    );
    assert!(stdout.contains("got 42"), "stdout={stdout} stderr={stderr}");
}

#[test]
fn child_process_spawn_with_net_keeps_host_id() {
    let script = write_temp_script(
        "spawn_net_id",
        r#"
const net = require('net');
const { spawn } = require('child_process');

const server = net.createServer();
const child = spawn(process.execPath, ['eval', '1 + 1'], {
  stdio: ['pipe', 'pipe', 'pipe', 'ipc'],
});

console.log('spawn-id', typeof child.__id, child.__id >= 0, child.connected);
const watchdog = setTimeout(function () { process.exit(4); }, 7000);
child.on('error', function (error) {
  console.log('spawn-error', error.message);
  clearTimeout(watchdog);
  process.exit(2);
});
child.on('exit', function (code) {
  console.log('spawn-exit', code);
  clearTimeout(watchdog);
  process.exit(code === 0 ? 0 : 3);
});
"#,
    );

    let output = run_wjsm_script(&script, &[]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status={:?} stdout={stdout} stderr={stderr}",
        output.status
    );
    assert!(
        stdout.contains("spawn-id number true true"),
        "stdout={stdout} stderr={stderr}"
    );
    assert!(stdout.contains("spawn-exit 0"), "stdout={stdout}");
}

#[test]
fn cluster_fork_message_and_exit() {
    let script = write_temp_script(
        "cluster_msg",
        r#"
const cluster = require('cluster');
if (cluster.isPrimary) {
  const w = cluster.fork();
  const watchdog = setTimeout(() => process.exit(2), 15000);
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
    clearTimeout(watchdog);
    process.exit(0);
  });
} else {
  process.on('message', (m) => {
    process.send({ reply: m.hello });
  });
}
"#,
    );

    let output = run_wjsm_script(&script, &[]);

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
///
/// 完成条件只认 `cluster` 的 `listening` 事件（IPC 事件驱动）：
/// 收齐 2 次 listening 且 port 相同即成功。
///
/// runtime 侧保证：
/// - listen / queryServer / RR bind 失败会 error 回包并让 worker 退出（无 error 监听者时 process.exit(1)）
/// - worker 未 listening 就 exit 时 primary 用 exit 事件收口，禁止永久挂起
/// - process.exit 会 SIGKILL 仍存活的子进程
///
/// 注意（wjsm 现状）：
/// - 不要在 listening 回调栈里同步 `worker.kill()` 再 `process.exit`。
/// - 不要用 `finished` 守卫再 `process.exit(code)`；直接 inline exit。
#[test]
fn cluster_net_shared_port_rr() {
    let script = write_temp_script(
        "cluster_net",
        r#"
const cluster = require('cluster');
const net = require('net');

if (cluster.isPrimary) {
  cluster.schedulingPolicy = cluster.SCHED_RR;
  var ports = [];
  var reports = 0;
  var shuttingDown = false;
  var exited = 0;

  function maybeFinish() {
    if (reports < 2) return;
    if (ports[0] > 0 && ports[0] === ports[1]) {
      console.log('shared-port ' + ports[0]);
      shuttingDown = true;
      var ids = Object.keys(cluster.workers);
      for (var i = 0; i < ids.length; i = i + 1) cluster.workers[ids[i]].kill();
      return;
    }
    console.log('port-mismatch ' + ports[0] + ' ' + ports[1]);
    process.exit(5);
  }

  function report(port, workerId, tag) {
    ports.push(port);
    reports = reports + 1;
    console.log(tag, workerId, port);
    maybeFinish();
  }

  cluster.on('listening', function (worker, addr) {
    worker.__gotListen = true;
    report(addr.port, worker.id, 'listening');
  });

  // bind/IPC 失败会抬到 cluster 'error'：直接失败退出，禁止挂死。
  cluster.on('error', function (err) {
    console.log('cluster-error', err && err.message);
    process.exit(6);
  });

  // 子进程若未 listening 就退出：用 exit 事件收口，禁止 primary 永久挂起。
  cluster.on('exit', function (worker, code, signal) {
    if (shuttingDown) {
      exited = exited + 1;
      if (exited === 2) process.exit(0);
      return;
    }
    if (worker.__gotListen) return;
    worker.__gotListen = true;
    report(-1, worker.id, 'exit-before-listen:' + code + ':' + signal);
  });

  cluster.fork();
  cluster.fork();
} else {
  net.createServer(function (socket) {
    socket.end('ok-' + process.pid);
  }).listen(0, '127.0.0.1');
}
"#,
    );

    let output = run_wjsm_script(&script, &[("NODE_CLUSTER_SCHED_POLICY", "rr")]);

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
    let listening_count = stdout.matches("listening ").count();
    assert!(
        listening_count >= 2,
        "expected >=2 listening events, got {listening_count}: {stdout}"
    );
}

/// RR primary 侧 connect 到共享端口。
/// 完成条件：listening + worker message(handled) + client end（纯事件）。
#[test]
fn cluster_rr_primary_net_connect() {
    let script = write_temp_script(
        "cluster_net_client",
        r#"
const cluster = require('cluster');
const net = require('net');

if (cluster.isPrimary) {
  cluster.schedulingPolicy = cluster.SCHED_RR;
  const worker = cluster.fork();
  var handled = false;
  var clientEnded = false;

  function maybeDone() {
    if (!handled || !clientEnded) return;
    process.exit(0);
  }

  worker.on('message', function (message) {
    if (message && message.kind === 'handled') {
      console.log('worker-handled');
      handled = true;
      maybeDone();
    }
  });

  cluster.on('listening', function (_worker, address) {
    console.log('listening-port', address.port);
    const client = net.connect(address.port, '127.0.0.1');
    client.on('end', function () {
      console.log('client-end');
      clientEnded = true;
      maybeDone();
    });
    client.on('error', function (error) {
      console.log('client-error', error.message);
      process.exit(7);
    });
  });
} else {
  net.createServer(function (socket) {
    process.send({ kind: 'handled' });
    socket.end();
  }).listen(0, '127.0.0.1');
}
"#,
    );

    let output = run_wjsm_script(&script, &[("NODE_CLUSTER_SCHED_POLICY", "rr")]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status={:?} stdout={stdout} stderr={stderr}",
        output.status
    );
    assert!(
        stdout.contains("listening-port"),
        "stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("worker-handled"),
        "stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("client-end"),
        "stdout={stdout} stderr={stderr}"
    );
}

/// listen 失败必须 emit error / worker 退出，禁止 UnhandledPromiseRejection 后挂死。
#[test]
fn net_listen_error_emits_and_exits() {
    let script = write_temp_script(
        "listen_error",
        r#"
const net = require('net');
const s = net.createServer();
s.on('error', function (err) {
  console.log('listen-error', err && err.message);
  process.exit(0);
});
// 非 root 绑 80 应失败
s.listen(80, '127.0.0.1');
"#,
    );
    let output = run_wjsm_script(&script, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status={:?} stdout={stdout} stderr={stderr}",
        output.status
    );
    assert!(
        stdout.contains("listen-error"),
        "expected listen-error in stdout={stdout} stderr={stderr}"
    );
}

/// cluster worker listen 失败时 worker 必须退出，primary 用 exit 收口，禁止 60s 挂死。
#[test]
fn cluster_worker_listen_failure_primary_exits() {
    let script = write_temp_script(
        "cluster_listen_fail",
        r#"
const cluster = require('cluster');
const net = require('net');

if (cluster.isPrimary) {
  cluster.schedulingPolicy = cluster.SCHED_RR;
  var exits = 0;
  cluster.on('exit', function (worker, code, signal) {
    exits = exits + 1;
    console.log('worker-exit', worker.id, code, signal);
    if (exits >= 1) {
      console.log('primary-done');
      process.exit(0);
    }
  });
  cluster.on('listening', function () {
    console.log('unexpected-listening');
    process.exit(3);
  });
  cluster.fork();
} else {
  // exclusive 强制走本地 bind，绕过 RR；绑 80 失败 → process.exit(1)
  net.createServer().listen({ port: 80, host: '127.0.0.1', exclusive: true });
}
"#,
    );
    let output = run_wjsm_script(&script, &[("NODE_CLUSTER_SCHED_POLICY", "rr")]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status={:?} stdout={stdout} stderr={stderr}",
        output.status
    );
    assert!(
        stdout.contains("primary-done"),
        "expected primary-done in stdout={stdout} stderr={stderr}"
    );
    assert!(
        !stdout.contains("unexpected-listening"),
        "should not listen on privileged port: {stdout}"
    );
}
