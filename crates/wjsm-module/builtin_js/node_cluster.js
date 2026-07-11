/**
 * Node.js-compatible `cluster` module（wjsm）。
 * Primary: fork workers via child_process.fork + IPC。
 * Worker: NODE_UNIQUE_ID；listen 走 RR / SO_REUSEPORT。
 */
import { EventEmitter } from 'events';

const SCHED_NONE = 1;
const SCHED_RR = 2;

function envSchedPolicy() {
  const v = process.env.NODE_CLUSTER_SCHED_POLICY;
  if (v === 'none') return SCHED_NONE;
  if (v === 'rr') return SCHED_RR;
  // Linux 默认 RR
  return SCHED_RR;
}

let schedulingPolicy = envSchedPolicy();
const workers = Object.create(null);
let workerIdSeq = 0;
const settings = {
  exec: undefined,
  execArgv: undefined,
  args: undefined,
  silent: false,
};

const uniqueId = process.env.NODE_UNIQUE_ID;
const isWorker = uniqueId !== undefined && uniqueId !== '';
const isPrimary = !isWorker;
// 删除以免再 fork 时错误继承
if (isWorker) {
  try {
    delete process.env.NODE_UNIQUE_ID;
  } catch (e) {}
}

// ── Worker 类 ─────────────────────────────────────────────────────────
export function Worker(options) {
  EventEmitter.call(this);
  options = options || {};
  this.id = options.id;
  this.process = options.process;
  this.exitedAfterDisconnect = false;
  this.state = 'none';
  this.__listening = null;
  const self = this;
  if (this.process) {
    // primary 侧 process 是 ChildProcess（EventEmitter）；worker 侧是 host process。
    // host process.on 仅保留单一 message_cb，不能在 Worker 构造器里抢先注册，
    // 否则覆盖 cluster worker 的 NODE_* 分流回调。
    const isHostProcess = this.process === process;
    if (!isHostProcess) {
      this.process.on('message', function (msg, handle) {
        self.emit('message', msg, handle);
      });
      this.process.on('exit', function (code, signal) {
        self.state = 'dead';
        removeWorkerFromEntries(self);
        self.emit('exit', code, signal);
        clusterEmit('exit', self, code, signal);
        delete workers[self.id];
      });
      this.process.on('disconnect', function () {
        self.state = 'disconnected';
        self.emit('disconnect');
        clusterEmit('disconnect', self);
      });
      // 内部协议（ChildProcess 是普通 JS 对象，可挂属性）
      this.process.__wjsm_internal_message = function (msg, fd) {
        handlePrimaryInternal(self, msg, fd);
      };
    }
  }
}
Worker.prototype = Object.create(EventEmitter.prototype);
Worker.prototype.constructor = Worker;

Worker.prototype.send = function (message, sendHandle, callback) {
  if (!this.process) return false;
  const ok = this.process.send(message, sendHandle);
  if (typeof sendHandle === 'function') callback = sendHandle;
  if (callback) setTimeout(function () {
    callback(null);
  }, 0);
  return ok;
};

Worker.prototype.kill = function (signal) {
  this.destroy(signal);
};

Worker.prototype.destroy = function (signal) {
  if (this.process) this.process.kill(signal || 'SIGTERM');
};

Worker.prototype.disconnect = function () {
  this.exitedAfterDisconnect = true;
  if (this.process) {
    this.process.send({ cmd: 'NODE_CLUSTER', act: 'disconnect' });
    this.process.disconnect();
  }
  return this;
};

Worker.prototype.isDead = function () {
  return this.state === 'dead';
};

Worker.prototype.isConnected = function () {
  return this.process && this.process.connected;
};

function installClusterInternalDispatch(child) {
  let internalHandler = null;
  const pendingInternal = [];
  Object.defineProperty(child, '__wjsm_internal_message', {
    get: function () { return internalHandler; },
    set: function (handler) {
      internalHandler = handler;
      if (typeof internalHandler !== 'function') return;
      const pending = pendingInternal.slice();
      pendingInternal.length = 0;
      for (var i = 0; i < pending.length; i = i + 1) {
        internalHandler(pending[i].msg, pending[i].fd);
      }
    },
    configurable: true,
  });
  child.__dispatchInternal = function (msg, fd) {
    if (typeof internalHandler === 'function') internalHandler(msg, fd);
    else pendingInternal.push({ msg: msg, fd: fd });
  };
}

function sendHandleFd(sendHandle) {
  if (typeof sendHandle === 'number') return sendHandle;
  if (sendHandle && sendHandle.__rawFd !== undefined) return sendHandle.__rawFd;
  return undefined;
}

function installClusterProcessMethods(child, created, host) {
  child.send = function (message, sendHandle) {
    if (!child.connected) return false;
    host.send(created.id, message, sendHandleFd(sendHandle));
    return true;
  };
  child.kill = function (signal) {
    child.killed = true;
    host.kill(created.id, signal || 'SIGTERM');
    return true;
  };
  child.disconnect = function () {
    if (!child.connected) return;
    host.disconnect(created.id);
    child.connected = false;
    child.emit('disconnect');
  };
}

function createClusterProcess(created, host) {
  const child = new EventEmitter();
  child.__id = created.id;
  child.pid = created.pid;
  child.connected = true;
  child.killed = false;
  child.exitCode = null;
  child.signalCode = null;
  installClusterInternalDispatch(child);
  installClusterProcessMethods(child, created, host);
  return child;
}

// ── cluster EventEmitter ──────────────────────────────────────────────
const clusterEE = new EventEmitter();
function clusterEmit() {
  // 直接传 arguments；slice.call(arguments) 在当前 lowerer 下可能得到错误数组
  return EventEmitter.prototype.emit.apply(clusterEE, arguments);
}

// ── Primary: RoundRobin handles ───────────────────────────────────────
// port0 专用单例 entry：port===0 时所有 worker 必须共享同一 listener。
// （动态 key 对象/数组查找在部分路径下不可靠，port0 是 cluster 最常见路径。）
let port0Entry = null;
const rrHandleList = [];

function handleKey(address, port, addressType, fd, flags) {
  return String(address) + '|' + String(port) + '|' + String(addressType) + '|' + String(fd) + '|' + String(flags);
}

function findRrEntry(key) {
  for (var i = 0; i < rrHandleList.length; i = i + 1) {
    const e = rrHandleList[i];
    if (e && e.key === key) return e;
  }
  return null;
}

function makeRrEntry(key, msg) {
  return {
    key: key,
    address: msg.address || '0.0.0.0',
    port: msg.port === undefined || msg.port === null ? 0 : msg.port,
    primaryWorker: null,
    workers: [],
    rrIndex: 0,
    listening: false,
    binding: false,
    closed: false,
    reusePort: schedulingPolicy === SCHED_NONE,
    boundPort: 0,
    boundAddress: '',
    serverHandle: undefined,
    waitList: [],
  };
}

function errorMessage(err, fallback) {
  if (err && typeof err === 'object' && err.message !== undefined) return String(err.message);
  if (err !== undefined && err !== null && err !== '') return String(err);
  return fallback || 'cluster error';
}

function sendClusterError(worker, seq, message) {
  if (!worker || !worker.process || typeof worker.process.send !== 'function') return;
  worker.process.send({
    cmd: 'NODE_CLUSTER',
    act: 'error',
    seq: seq,
    message: message,
  });
}

/** bind 失败 / worker 中途退出：把 waitList 全部 error 回包，并清 binding。 */
function failEntryWaiters(entry, message, primaryWorker, primarySeq) {
  if (!entry) return;
  entry.binding = false;
  entry.listening = false;
  const waiters = entry.waitList;
  entry.waitList = [];
  const seen = Object.create(null);
  if (primaryWorker) {
    sendClusterError(primaryWorker, primarySeq, message);
    if (primarySeq !== undefined && primarySeq !== null) seen[String(primarySeq)] = true;
  }
  for (var i = 0; i < waiters.length; i = i + 1) {
    const item = waiters[i];
    if (!item) continue;
    const key = String(item.seq);
    if (seen[key]) continue;
    seen[key] = true;
    sendClusterError(item.worker, item.seq, message);
  }
  clusterEmit('error', new Error(message));
}

function closeRrEntry(entry, netHost) {
  if (!entry || entry.closed) return;
  entry.closed = true;
  entry.listening = false;
  entry.binding = false;
  const handle = entry.serverHandle;
  entry.serverHandle = undefined;
  if (handle !== undefined && handle !== null && netHost && typeof netHost.serverClose === 'function') {
    try {
      netHost.serverClose(handle);
    } catch (_e) {}
  }
}

/** worker 退出时从所有 RR entry 摘掉；无人时关闭 primary 持有的 listener。 */
function removeWorkerFromEntries(worker) {
  if (!worker) return;
  const netHost = globalThis.__wjsm_node_net;
  for (var i = 0; i < rrHandleList.length; i = i + 1) {
    const entry = rrHandleList[i];
    if (!entry) continue;
    const nextWorkers = [];
    for (var j = 0; j < entry.workers.length; j = j + 1) {
      const w = entry.workers[j];
      if (w && w.id !== worker.id) nextWorkers.push(w);
    }
    entry.workers = nextWorkers;
    if (entry.primaryWorker && entry.primaryWorker.id === worker.id) {
      entry.primaryWorker = nextWorkers.length > 0 ? nextWorkers[0] : null;
    }
    const nextWait = [];
    for (var k = 0; k < entry.waitList.length; k = k + 1) {
      const item = entry.waitList[k];
      if (!item || !item.worker) continue;
      if (item.worker.id === worker.id) {
        sendClusterError(item.worker, item.seq, 'worker exited during listen');
        continue;
      }
      nextWait.push(item);
    }
    entry.waitList = nextWait;
    if (nextWorkers.length === 0 && entry.serverHandle !== undefined) {
      closeRrEntry(entry, netHost);
    }
  }
}

function handlePrimaryInternal(worker, msg, fd) {
  if (!msg || msg.cmd !== 'NODE_CLUSTER') return;
  if (msg.act === 'online') {
    worker.state = 'online';
    worker.emit('online');
    clusterEmit('online', worker);
    return;
  }
  if (msg.act === 'queryServer') {
    handleQueryServer(worker, msg);
    return;
  }
  if (msg.act === 'listening') {
    worker.state = 'listening';
    worker.__listening = msg.address;
    worker.emit('listening', msg.address);
    clusterEmit('listening', worker, msg.address);
    return;
  }
  if (msg.act === 'close') {
    // worker 关闭了共享 server
    return;
  }
}

function handleQueryServer(worker, msg) {
  const key = handleKey(msg.address, msg.port, msg.addressType, msg.fd, msg.flags);
  const portNum = msg.port === undefined || msg.port === null ? 0 : Number(msg.port);
  let entry;
  // port 0：强制单例共享
  if (portNum === 0 || !portNum) {
    if (!port0Entry) {
      port0Entry = makeRrEntry(key, msg);
      rrHandleList.push(port0Entry);
    }
    entry = port0Entry;
  } else {
    entry = findRrEntry(key);
    if (!entry) {
      entry = makeRrEntry(key, msg);
      rrHandleList.push(entry);
    }
  }
  registerServerWorker(entry, worker);

  if (schedulingPolicy === SCHED_NONE) {
    if (!entry.listening && !entry.binding) {
      entry.binding = true;
      const netHost = globalThis.__wjsm_node_net;
      const listenPort = normalizeListenPort(entry.port);
      const listenHost = normalizeListenHost(entry.address);
      const replySeq = msg.seq;
      const replyWorker = worker;
      if (!netHost || typeof netHost.serverListen !== 'function') {
        failEntryWaiters(entry, 'net host unavailable', replyWorker, replySeq);
        return;
      }
      // 单参 .then + .catch：双参 then 在 lowerer 下会丢 rejection 路径
      netHost.serverListen(listenPort, listenHost, { reusePort: true }).then(function (handle) {
        entry.listening = true;
        entry.binding = false;
        entry.boundPort = netHost.serverPort(handle);
        entry.boundAddress = netHost.serverAddress(handle);
        netHost.serverClose(handle);
        notifyShare(entry, replySeq, replyWorker);
        const waiters = entry.waitList;
        entry.waitList = [];
        for (var si = 0; si < waiters.length; si = si + 1) {
          notifyShare(entry, waiters[si].seq, waiters[si].worker);
        }
      }).catch(function (err) {
        failEntryWaiters(
          entry,
          errorMessage(err, 'cluster share listen failed'),
          replyWorker,
          replySeq
        );
      });
    } else if (entry.listening) {
      notifyShare(entry, msg.seq, worker);
    } else {
      entry.waitList.push({ worker: worker, seq: msg.seq });
    }
    return;
  }

  // SCHED_RR
  if (entry.closed) {
    sendClusterError(worker, msg.seq, 'cluster handle closed');
    return;
  }
  if (entry.listening) {
    sendRrReply(
      worker,
      msg.seq,
      entry.boundAddress || entry.address || '127.0.0.1',
      entry.boundPort > 0 ? entry.boundPort : entry.port
    );
    return;
  }
  entry.waitList.push({ worker: worker, seq: msg.seq });
  if (entry.binding) return;
  startRoundRobin(entry, worker, msg.seq);

}

function normalizeListenPort(port) {
  const bindPort = Number(port);
  if (bindPort !== bindPort || bindPort < 0 || bindPort > 65535) return 0;
  return bindPort;
}

function normalizeListenHost(address) {
  const bindHost = String(address || '127.0.0.1');
  if (bindHost === '0.0.0.0' || bindHost === '::' || bindHost === '') return '127.0.0.1';
  return bindHost;
}

function sendRrReply(worker, seq, address, port) {
  if (!worker || !worker.process) return;
  worker.process.send({
    cmd: 'NODE_CLUSTER',
    act: 'rr',
    seq: seq,
    address: {
      address: address,
      port: port,
      family: 'IPv4',
    },
  });
}

function notifyShare(entry, seq, worker) {
  const payload = {
    cmd: 'NODE_CLUSTER',
    act: 'share',
    seq: seq,
    address: entry.boundAddress || entry.address,
    port: entry.boundPort !== undefined ? entry.boundPort : entry.port,
    reusePort: true,
  };
  if (worker) {
    worker.process.send(payload);
  } else {
    for (var i = 0; i < entry.workers.length; i = i + 1) {
      entry.workers[i].process.send(Object.assign({}, payload, { seq: undefined }));
    }
  }
}

function registerServerWorker(entry, worker) {
  for (var i = 0; i < entry.workers.length; i = i + 1) {
    if (entry.workers[i] && entry.workers[i].id === worker.id) return;
  }
  entry.workers[entry.workers.length] = worker;
}

function startRoundRobin(entry, firstWorker, firstSeq) {
  registerServerWorker(entry, firstWorker);
  // 捕获的 workers 数组尚未 materialize 时，首个 listener 仍可接收连接。
  entry.primaryWorker = firstWorker;
  const netHost = globalThis.__wjsm_node_net;
  if (!netHost || typeof netHost.serverListen !== 'function') {
    failEntryWaiters(entry, 'net host unavailable', firstWorker, firstSeq);
    return;
  }
  const listenPort = normalizeListenPort(entry.port);
  const listenHost = normalizeListenHost(entry.address);
  entry.binding = true;
  entry.closed = false;
  // 闭包捕获发起者，确保 then 回调内一定能回包（不依赖 entry 动态字段）
  const replyWorker = firstWorker;
  const replySeq = firstSeq;
  // 单参 .then + .catch：双参 then 会把 rejection 路径弄丢，导致 binding 永久 true
  netHost.serverListen(listenPort, listenHost, { reusePort: false }).then(function (handle) {
    if (entry.closed) {
      try {
        netHost.serverClose(handle);
      } catch (_e) {}
      return;
    }
    const boundPort = netHost.serverPort(handle);
    const boundAddress = netHost.serverAddress(handle);
    entry.listening = true;
    entry.binding = false;
    entry.serverHandle = handle;
    entry.boundPort = boundPort;
    entry.boundAddress = boundAddress;
    // 1) 发起者（用局部变量，不读 entry 后加字段）
    sendRrReply(replyWorker, replySeq, boundAddress, boundPort);
    // 2) waitList 里其他人
    const waiters = entry.waitList;
    entry.waitList = [];
    for (var i = 0; i < waiters.length; i = i + 1) {
      const item = waiters[i];
      if (!item) continue;
      if (item.seq === replySeq) continue;
      sendRrReply(item.worker, item.seq, boundAddress, boundPort);
    }
    acceptLoop(entry, netHost);
  }).catch(function (err) {
    failEntryWaiters(
      entry,
      errorMessage(err, 'cluster round-robin listen failed'),
      replyWorker,
      replySeq
    );
  });
}

function acceptLoop(entry, netHost) {
  if (!entry || entry.closed) return;
  if (entry.serverHandle === undefined || entry.serverHandle === null) return;
  const acceptPromise = netHost.serverAcceptRawFd
    ? netHost.serverAcceptRawFd(entry.serverHandle)
    : netHost.serverAccept(entry.serverHandle);
  // 单参 then + catch：避免双参 then 的 lowerer 控制流 bug
  acceptPromise.then(function (rawOrHandle) {
    if (entry.closed) return;
    if (rawOrHandle === null || rawOrHandle === undefined) {
      scheduleAccept(entry, netHost);
      return;
    }
    const workerCount = entry.workers.length;
    const worker = workerCount > 0
      ? entry.workers[entry.rrIndex % workerCount]
      : entry.primaryWorker;
    if (!worker || !worker.process || !worker.process.connected) {
      scheduleAccept(entry, netHost);
      return;
    }
    entry.rrIndex = entry.rrIndex + 1;
    worker.process.send({ cmd: 'NODE_CLUSTER', act: 'newconn' }, rawOrHandle);
    scheduleAccept(entry, netHost);
  }).catch(function (error) {
    if (entry.closed) return;
    clusterEmit(
      'error',
      error instanceof Error ? error : new Error(errorMessage(error, 'accept failed'))
    );
    // accept 瞬时失败不拆 listener，继续轮询；closeRrEntry 后停止
    scheduleAccept(entry, netHost);
  });
}

function scheduleAccept(entry, netHost) {
  if (!entry || entry.closed) return;
  setTimeout(function () {
    acceptLoop(entry, netHost);
  }, 0);
}

// ── Worker 侧 ─────────────────────────────────────────────────────────
function createCurrentWorker() {
  if (!isWorker) return null;
  return new Worker({ id: Number(uniqueId), process: process });
}

let workerObj = createCurrentWorker();
const pendingListens = Object.create(null);
let listenSeq = 0;

function clearPendingListen(seq, settle) {
  const pending = pendingListens[seq];
  if (!pending) return false;
  delete pendingListens[seq];
  clearTimeout(pending.timeout);
  if (typeof settle === 'function') settle(pending);
  return true;
}

function clearAllPendingListens(reason) {
  const keys = Object.keys(pendingListens);
  for (let i = 0; i < keys.length; i = i + 1) {
    const seq = keys[i];
    clearPendingListen(seq, function (pending) {
      pending.reject(new Error(reason || 'cluster listen cancelled'));
    });
  }
}

if (isWorker) {
  workerObj.state = 'online';
  // host process.on：注册唯一 message_cb，分流 NODE_* / 用户消息
  if (typeof process.on === 'function') {
    process.on('message', function (msg, fd) {
      if (msg && typeof msg === 'object' && typeof msg.cmd === 'string' && msg.cmd.indexOf('NODE_') === 0) {
        handleWorkerInternal(msg, fd);
        return;
      }
      if (workerObj) workerObj.emit('message', msg, fd);
    });
    // disconnect / 进程退出时清掉未完成的 queryServer
    process.on('disconnect', function () {
      clearAllPendingListens('worker disconnect');
    });
    process.on('exit', function () {
      clearAllPendingListens('process exit');
    });
  }
  // 通知 primary online（process_send 会 ensure endpoint + reader）
  if (typeof process.send === 'function') {
    process.send({ cmd: 'NODE_CLUSTER', act: 'online' });
  }
}

function handleWorkerInternal(msg, fd) {
  if (!msg || msg.cmd !== 'NODE_CLUSTER') return;
  if (msg.act === 'share' || msg.act === 'rr') {
    clearPendingListen(msg.seq, function (pending) {
      pending.resolve(msg);
    });
    return;
  }
  if (msg.act === 'newconn') {
    // fd 是 raw socket fd；交给 net
    const netHost = globalThis.__wjsm_node_net;
    if (netHost && typeof netHost.socketFromFd === 'function' && fd !== undefined) {
      const socketHandle = netHost.socketFromFd(fd);
      if (workerObj && workerObj.__serverConnectionHandler) {
        workerObj.__serverConnectionHandler(socketHandle);
      }
    }
    return;
  }
  if (msg.act === 'disconnect') {
    clearAllPendingListens('worker disconnect');
    if (typeof process.disconnect === 'function') process.disconnect();
    return;
  }
  if (msg.act === 'error') {
    clearPendingListen(msg.seq, function (pending) {
      pending.reject(new Error(msg.message || 'cluster listen error'));
    });
  }
}

/**
 * Worker 上 net.Server.listen 调用：向 primary 查询共享策略。
 * 返回 Promise<{ mode: 'share'|'rr', address, reusePort? }>
 */
export function queryServerListen(opts) {
  if (!isWorker) {
    return Promise.resolve({ mode: 'local' });
  }
  const seq = (listenSeq = listenSeq + 1);
  return new Promise(function (resolve, reject) {
    const timeout = setTimeout(function () {
      clearPendingListen(seq, function (pending) {
        pending.reject(new Error('cluster queryServer timeout'));
      });
    }, 10000);
    pendingListens[seq] = { resolve: resolve, reject: reject, timeout: timeout };
    process.send({
      cmd: 'NODE_CLUSTER',
      act: 'queryServer',
      seq: seq,
      address: opts.host || '0.0.0.0',
      port: opts.port || 0,
      addressType: 4,
      fd: -1,
      flags: 0,
    });
  });
}

/** Worker 注册 connection 投递回调（RR 模式）。 */
export function registerConnectionHandler(handler) {
  if (workerObj) workerObj.__serverConnectionHandler = handler;
}

// ── Public API ────────────────────────────────────────────────────────
export function setupPrimary(options) {
  options = options || {};
  if (options.exec !== undefined) settings.exec = options.exec;
  if (options.execArgv !== undefined) settings.execArgv = options.execArgv;
  if (options.args !== undefined) settings.args = options.args;
  if (options.silent !== undefined) settings.silent = options.silent;
}
export const setupMaster = setupPrimary;

/**
 * 只传覆盖项。spawn 宿主侧会先继承 process.env 再叠 envPairs，
 * 避免 Object.assign(process.env) / 全量拷贝触发 env Proxy 不变量错误。
 */
function overlayEnv(extra) {
  const out = {};
  if (extra && typeof extra === 'object') {
    const ek = Object.keys(extra);
    for (var j = 0; j < ek.length; j = j + 1) {
      out[ek[j]] = extra[ek[j]];
    }
  }
  return out;
}

export function fork(env) {
  if (!isPrimary) {
    throw new Error('cluster.fork can only be called from primary');
  }
  workerIdSeq = workerIdSeq + 1;
  const id = workerIdSeq;
  const childEnv = overlayEnv(env || {});
  childEnv.NODE_UNIQUE_ID = String(id);
  const exec = settings.exec || process.argv[1];
  if (!exec || typeof exec !== 'string') {
    throw new Error('cluster.fork: no exec path (settings.exec / process.argv[1])');
  }
  const args = Array.isArray(settings.args) ? settings.args : [];
  const execPath = typeof process.execPath === 'string' && process.execPath
    ? process.execPath
    : 'wjsm';
  const commandArgs = ['run', String(exec)];
  if (Array.isArray(args)) {
    for (var ai = 0; ai < args.length; ai = ai + 1) {
      if (args[ai] !== undefined && args[ai] !== null) commandArgs.push(String(args[ai]));
    }
  }
  // 直接调 host，绕过 child_process.spawn JS 包装（与 net 同捆时 wrapper 有 id 丢失回归）
  const cpHost = globalThis.__wjsm_node_child_process;
  if (!cpHost || typeof cpHost['spawn'] !== 'function') {
    throw new Error('cluster.fork: child_process host unavailable');
  }
  const envPairs = [];
  const envKeys = Object.keys(childEnv);
  for (var ei = 0; ei < envKeys.length; ei = ei + 1) {
    envPairs.push(envKeys[ei] + '=' + String(childEnv[envKeys[ei]]));
  }
  const created = cpHost['spawn'](execPath, commandArgs, {
    envPairs: envPairs,
    ipc: true,
  });
  if (!created || typeof created.id !== 'number') {
    throw new Error('cluster.fork: host spawn failed');
  }
  const child = createClusterProcess(created, cpHost);
  cpHost['onMessage'](created.id, function (msg, fd) {
    if (msg && typeof msg === 'object' && typeof msg.cmd === 'string' && msg.cmd.indexOf('NODE_') === 0) {
      child.__dispatchInternal(msg, fd);
      return;
    }
    child.emit('message', msg, fd);
  });
  cpHost['onExit'](created.id, function (code, signal) {
    child.connected = false;
    child.exitCode = code;
    child.signalCode = signal;
    child.emit('exit', code, signal);
    child.emit('close', code, signal);
  });
  const worker = new Worker({ id: id, process: child });
  if (worker.id === undefined) {
    throw new Error('cluster.fork: Worker construction failed');
  }
  worker.state = 'none';
  workers[id] = worker;
  return worker;
}

export function disconnect(callback) {
  const ids = Object.keys(workers);
  if (ids.length === 0) {
    if (callback) setTimeout(callback, 0);
    return;
  }
  let left = ids.length;
  for (var i = 0; i < ids.length; i = i + 1) {
    workers[ids[i]].disconnect();
    workers[ids[i]].once('disconnect', function () {
      left = left - 1;
      if (left === 0 && callback) callback();
    });
  }
}

export function on(event, listener) {
  return clusterEE.on(event, listener);
}
export function once(event, listener) {
  return clusterEE.once(event, listener);
}
export function off(event, listener) {
  return clusterEE.off ? clusterEE.off(event, listener) : clusterEE.removeListener(event, listener);
}
export function removeListener(event, listener) {
  return clusterEE.removeListener(event, listener);
}
export function emit() {
  return EventEmitter.prototype.emit.apply(clusterEE, arguments);
}

export { isPrimary, isWorker, workers, settings, SCHED_NONE, SCHED_RR };
export const isMaster = isPrimary;
export const worker = workerObj;

// 用数据属性暴露 schedulingPolicy（CJS interop 可能丢 getter/setter）
const cluster = {
  isPrimary: isPrimary,
  isMaster: isMaster,
  isWorker: isWorker,
  Worker: Worker,
  workers: workers,
  worker: workerObj,
  settings: settings,
  SCHED_NONE: SCHED_NONE,
  SCHED_RR: SCHED_RR,
  schedulingPolicy: schedulingPolicy,
  setupPrimary: setupPrimary,
  setupMaster: setupMaster,
  fork: fork,
  disconnect: disconnect,
  on: on,
  once: once,
  off: off,
  removeListener: removeListener,
  emit: emit,
  queryServerListen: queryServerListen,
  registerConnectionHandler: registerConnectionHandler,
};
// 赋值时同步回模块内变量
Object.defineProperty(cluster, 'schedulingPolicy', {
  get: function () {
    return schedulingPolicy;
  },
  set: function (v) {
    schedulingPolicy = v;
  },
  enumerable: true,
  configurable: true,
});

// 供 net.Server.listen 发现 cluster worker 上下文，避免 net↔cluster 循环 import
globalThis.__wjsm_cluster = cluster;

export default cluster;
