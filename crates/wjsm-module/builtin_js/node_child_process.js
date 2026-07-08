import { EventEmitter } from 'events';
import { Readable, Writable } from 'stream';

function getHost() {
  const host = globalThis.__wjsm_node_child_process;
  if (!host) throw new Error('wjsm internal child_process host bridge is not installed');
  return host;
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

export function ChildProcess(result) {
  EventEmitter.call(this);
  this.pid = result && result.pid ? result.pid : 0;
  this.exitCode = result ? result.status : null;
  this.signalCode = result ? result.signal : null;
  this.stdin = new Writable();
  this.stdout = Readable.from(result && result.stdout ? [result.stdout] : []);
  this.stderr = Readable.from(result && result.stderr ? [result.stderr] : []);
  this.killed = false;
}
ChildProcess.prototype.kill = function (signal) {
  this.killed = true;
  this.signalCode = signal || 'SIGTERM';
  this.emit('exit', this.exitCode, this.signalCode);
  this.emit('close', this.exitCode, this.signalCode);
  return true;
};

export function spawnSync(command, args, options) {
  if (!Array.isArray(args)) { options = args; args = []; }
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
  if (!Array.isArray(args)) { options = args; args = []; }
  let result;
  try {
    result = getHost().spawnSync(String(command), args || [], normalizeOptions(options));
  } catch (error) {
    const failed = new ChildProcess({ stdout: Buffer.from(''), stderr: Buffer.from(''), status: null, signal: null, pid: 0 });
    setTimeout(() => failed.emit('error', error), 0);
    return failed;
  }
  const child = new ChildProcess(result);
  setTimeout(() => {
    child.emit('spawn');
    child.emit('exit', result.status, result.signal);
    child.emit('close', result.status, result.signal);
  }, 0);
  return child;
}

export function exec(command, options, callback) {
  if (typeof options === 'function') { callback = options; options = {}; }
  const normalized = normalizeOptions(options);
  let result;
  try {
    result = getHost().spawnSync(String(command), [], { cwd: normalized.cwd, envPairs: normalized.envPairs, encoding: 'buffer', timeout: normalized.timeout, maxBuffer: normalized.maxBuffer, shell: true, input: normalized.input });
  } catch (error) {
    if (callback) setTimeout(() => callback(error, undefined, undefined), 0);
    const failed = new ChildProcess({ stdout: Buffer.from(''), stderr: Buffer.from(''), status: null, signal: null, pid: 0 });
    setTimeout(() => failed.emit('error', error), 0);
    return failed;
  }
  const stdout = decodeOutput(result.stdout, normalized.encoding);
  const stderr = decodeOutput(result.stderr, normalized.encoding);
  const error = makeError(result, command);
  if (callback) setTimeout(() => callback(error, stdout, stderr), 0);
  const child = new ChildProcess(result);
  setTimeout(() => {
    if (error) child.emit('error', error);
    child.emit('exit', result.status, result.signal);
    child.emit('close', result.status, result.signal);
  }, 0);
  return child;
}

export function fork(modulePath, args, options) {
  if (!Array.isArray(args)) { options = args; args = []; }
  const commandArgs = [String(modulePath)];
  for (var i = 0; i < (args || []).length; i = i + 1) commandArgs.push(args[i]);
  return spawn(process.execPath || 'node', commandArgs, options);
}

const childProcess = { ChildProcess, spawnSync, execSync, spawn, exec, fork };
export default childProcess;
