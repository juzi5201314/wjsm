import { EventEmitter } from 'events';
import { Readable, Writable } from 'stream';

function getHost() {
  // 每次从 globalThis 读取；勿缓存引用（嵌套函数 $0.$global 历史上可能拿到不同对象）
  const host = globalThis.__wjsm_node_child_process;
  if (!host) throw new Error('wjsm internal child_process host bridge is not installed');
  return host;
}

// 通过 bracket 取出 host 方法，避免 `host.spawn` 被 lowerer 错误解析为对本模块 spawn 的递归调用
function hostSpawn(command, args, options) {
  const host = globalThis.__wjsm_node_child_process;
  const fn = host['spawn'];
  return fn.call(host, command, args, options);
}

function envPairs(env) {
  const pairs = [];
  if (!env || typeof env !== 'object') return pairs;
  const keys = Object.keys(env);
  for (var i = 0; i < keys.length; i = i + 1) pairs.push(keys[i] + '=' + String(env[keys[i]]));
  return pairs;
}

function normalizeOptions(options) {
  if (!options || typeof options !== 'object') options = {};
  return {
    cwd: options.cwd === undefined ? undefined : String(options.cwd),
    envPairs: envPairs(options.env),
    encoding: options.encoding === undefined ? 'buffer' : String(options.encoding),
    timeout: options.timeout === undefined ? 0 : Number(options.timeout),
    maxBuffer: options.maxBuffer === undefined ? 1024 * 1024 : Number(options.maxBuffer),
    shell: options.shell === undefined ? false : options.shell,
    input: options.input,
    ipc: Boolean(options.ipc),
  };
}

function decodeOutput(buffer, encoding) {
  if (!encoding || encoding === 'buffer') return buffer;
  return Buffer.from(buffer).toString(encoding);
}

function makeError(result, command) {
  if (result.status === 0) return null;
  const error = new Error('Command failed: ' + command);
  error.status = result.status;
  error.signal = result.signal;
  error.stdout = result.stdout;
  error.stderr = result.stderr;
  return error;
}

// process.send / process.on 由 host 在有 IPC 时安装。
// 注意：process 是 host 对象，JS 赋值 process.on / process.__wjsm_* 可能无效或
// 覆盖 host 方法后反而吞掉消息。不要在这里过滤 NODE_*——由注册方（cluster）
// 在自己的 process.on('message') 回调里分流。
// 父进程侧 ChildProcess 是普通 JS 对象，wireChild 里可以安全挂 __wjsm_internal_message。

// ── ChildProcess ──────────────────────────────────────────────────────
export function ChildProcess() {
  EventEmitter.call(this);
  this.pid = 0;
  this.__id = undefined;
  this.exitCode = null;
  this.signalCode = null;
  this.killed = false;
  this.connected = false;
  this.stdin = null;
  this.stdout = null;
  this.stderr = null;
}
ChildProcess.prototype = Object.create(EventEmitter.prototype);
ChildProcess.prototype.constructor = ChildProcess;

ChildProcess.prototype.kill = function (signal) {
  if (this.__id === undefined) {
    this.killed = true;
    this.signalCode = signal || 'SIGTERM';
    this.emit('exit', this.exitCode, this.signalCode);
    this.emit('close', this.exitCode, this.signalCode);
    return true;
  }
  this.killed = true;
  globalThis.__wjsm_node_child_process.kill(this.__id, signal || 'SIGTERM');
  return true;
};

ChildProcess.prototype.send = function (message, sendHandle) {
  if (this.__id === undefined || !this.connected) return false;
  var fd = undefined;
  if (typeof sendHandle === 'number') fd = sendHandle;
  else if (sendHandle && sendHandle.__rawFd !== undefined) fd = sendHandle.__rawFd;
  try {
    // host 接受 JS 对象并 JSON 序列化
    // 直接用 globalThis，避免 getHost 闭包/作用域下 $0.$global 异常
    const host = globalThis.__wjsm_node_child_process;
    if (!host || typeof host.send !== 'function') {
      throw new Error('wjsm internal child_process host bridge is not installed');
    }
    host.send(this.__id, message, fd);
    return true;
  } catch (e) {
    return false;
  }
};

ChildProcess.prototype.disconnect = function () {
  if (this.__id === undefined) return;
  globalThis.__wjsm_node_child_process.disconnect(this.__id);
  this.connected = false;
  this.emit('disconnect');
};

function wireChild(child, created) {
  child.__id = created.id;
  child.pid = created.pid;
  child.connected = true;
  const host = globalThis.__wjsm_node_child_process;
  host.onMessage(created.id, function (msg, fd) {
    if (msg && typeof msg === 'object' && typeof msg.cmd === 'string' && msg.cmd.indexOf('NODE_') === 0) {
      if (typeof child.__wjsm_internal_message === 'function') {
        child.__wjsm_internal_message(msg, fd);
      }
      return;
    }
    child.emit('message', msg, fd);
  });
  host.onExit(created.id, function (code, signal) {
    child.connected = false;
    child.exitCode = code;
    child.signalCode = signal;
    child.emit('exit', code, signal);
    child.emit('close', code, signal);
  });
  // spawn 事件异步
  setTimeout(function () {
    child.emit('spawn');
  }, 0);
  return child;
}

export function spawnSync(command, args, options) {
  if (!Array.isArray(args)) {
    options = args;
    args = [];
  }
  const normalized = normalizeOptions(options);
  const result = getHost().spawnSync(String(command), args || [], normalized);
  result.stdout = decodeOutput(result.stdout, normalized.encoding);
  result.stderr = decodeOutput(result.stderr, normalized.encoding);
  return result;
}

export function execSync(command, options) {
  const normalized = normalizeOptions(options);
  const out = getHost().execSync(String(command), normalized);
  return decodeOutput(out, normalized.encoding);
}

export function spawn(command, args, options) {
  if (!Array.isArray(args)) {
    options = args;
    args = [];
  }
  options = options || {};
  const normalized = normalizeOptions(options);
  // stdio 含 ipc 时启用 IPC
  if (options.stdio === 'ipc' || (Array.isArray(options.stdio) && options.stdio.indexOf('ipc') >= 0)) {
    normalized.ipc = true;
  }
  // 重建 argv 字符串数组，避免宿主侧数组元素被误读成 handle 数字
  const argv = [];
  if (Array.isArray(args)) {
    for (var i = 0; i < args.length; i = i + 1) {
      argv.push(String(args[i]));
    }
  }
  try {
    const created = hostSpawn(String(command), argv, normalized);
    if (!created || typeof created.id !== 'number') {
      const failed = new ChildProcess();
      setTimeout(function () {
        failed.emit('error', new Error('spawn returned no id'));
      }, 0);
      return failed;
    }
    const child = new ChildProcess();
    return wireChild(child, created);
  } catch (error) {
    const failed = new ChildProcess();
    setTimeout(function () {
      failed.emit('error', error);
    }, 0);
    return failed;
  }
}

export function exec(command, options, callback) {
  if (typeof options === 'function') {
    callback = options;
    options = {};
  }
  const normalized = normalizeOptions(options);
  let result;
  try {
    result = getHost().spawnSync(String(command), [], {
      cwd: normalized.cwd,
      envPairs: normalized.envPairs,
      encoding: 'buffer',
      timeout: normalized.timeout,
      maxBuffer: normalized.maxBuffer,
      shell: true,
      input: normalized.input,
    });
  } catch (error) {
    if (callback) setTimeout(function () {
      callback(error, undefined, undefined);
    }, 0);
    const failed = new ChildProcess();
    setTimeout(function () {
      failed.emit('error', error);
    }, 0);
    return failed;
  }
  const stdout = decodeOutput(result.stdout, normalized.encoding);
  const stderr = decodeOutput(result.stderr, normalized.encoding);
  const error = makeError(result, command);
  if (callback) setTimeout(function () {
    callback(error, stdout, stderr);
  }, 0);
  const child = new ChildProcess();
  child.exitCode = result.status;
  child.signalCode = result.signal;
  setTimeout(function () {
    if (error) child.emit('error', error);
    child.emit('exit', result.status, result.signal);
    child.emit('close', result.status, result.signal);
  }, 0);
  return child;
}

export function fork(modulePath, args, options) {
  if (!Array.isArray(args)) {
    options = args;
    args = [];
  }
  options = options || {};
  const execPath = typeof process.execPath === 'string' && process.execPath
    ? process.execPath
    : 'wjsm';
  // 固定 `run` 子命令：避免 process.execArgv / 选项数组在宿主侧被误读成数字 handle。
  const commandArgs = ['run', String(modulePath)];
  if (Array.isArray(args)) {
    for (var j = 0; j < args.length; j = j + 1) {
      if (args[j] !== undefined && args[j] !== null) {
        commandArgs.push(String(args[j]));
      }
    }
  }
  const spawnOpts = {
    cwd: options.cwd,
    env: options.env,
    encoding: options.encoding,
    timeout: options.timeout,
    maxBuffer: options.maxBuffer,
    shell: options.shell,
    input: options.input,
    stdio: options.stdio || ['pipe', 'pipe', 'pipe', 'ipc'],
    ipc: true,
  };
  return spawn(execPath, commandArgs, spawnOpts);
}

const childProcess = { ChildProcess: ChildProcess, spawnSync: spawnSync, execSync: execSync, spawn: spawn, exec: exec, fork: fork };
export default childProcess;
