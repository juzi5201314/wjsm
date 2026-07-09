import { EventEmitter } from 'events';

function getHost() {
  const host = globalThis.__wjsm_node_worker_threads;
  if (!host) throw new Error('wjsm internal worker_threads host bridge is not installed');
  return host;
}
const host = getHost();

function transferListOf(transferList) {
  return transferList === undefined ? [] : transferList;
}

function unwrapWorkerId(created) {
  if (created !== null && typeof created === 'object' && created.id !== undefined) return created.id;
  return created;
}

function unwrapWorkerThreadId(created) {
  if (created !== null && typeof created === 'object' && created.threadId !== undefined) {
    return created.threadId;
  }
  return -1;
}

function normalizeWorkerOptions(options) {
  if (!options || typeof options !== 'object') options = {};
  return {
    eval: Boolean(options.eval),
    workerData: options.workerData,
    name: options.name === undefined ? undefined : String(options.name),
    resourceLimits: options.resourceLimits,
    transferList: transferListOf(options.transferList),
    argv: options.argv,
    env: options.env,
    execArgv: options.execArgv,
    stdin: Boolean(options.stdin),
    stdout: Boolean(options.stdout),
    stderr: Boolean(options.stderr),
    trackUnmanagedFds: options.trackUnmanagedFds === undefined ? true : Boolean(options.trackUnmanagedFds),
  };
}

export function MessagePort(id) {
  EventEmitter.call(this);
  this.__id = id;
  this.__started = false;
  this.__closed = false;
}
MessagePort.prototype = Object.create(EventEmitter.prototype);
MessagePort.prototype.constructor = MessagePort;

MessagePort.prototype.postMessage = function (value, transferList) {
  if (this.__closed) return;
  host.portPostMessage(this.__id, value, transferListOf(transferList));
};

MessagePort.prototype.start = function () {
  if (this.__started || this.__closed) return this;
  this.__started = true;
  const port = this;
  host.portStart(this.__id, function (value) {
    if (!port.__closed) port.emit('message', value);
  });
  return this;
};

MessagePort.prototype.close = function () {
  if (this.__closed) return this;
  this.__closed = true;
  host.portClose(this.__id);
  this.emit('close');
  return this;
};

MessagePort.prototype.ref = function () {
  host.portRef(this.__id);
  return this;
};

MessagePort.prototype.unref = function () {
  host.portUnref(this.__id);
  return this;
};

MessagePort.prototype.on = function (name, listener) {
  const result = EventEmitter.prototype.on.call(this, name, listener);
  if (name === 'message') this.start();
  return result;
};
MessagePort.prototype.addListener = MessagePort.prototype.on;

MessagePort.prototype.once = function (name, listener) {
  const result = EventEmitter.prototype.once.call(this, name, listener);
  if (name === 'message') this.start();
  return result;
};

MessagePort.prototype.prependListener = function (name, listener) {
  const result = EventEmitter.prototype.prependListener.call(this, name, listener);
  if (name === 'message') this.start();
  return result;
};

MessagePort.prototype.prependOnceListener = function (name, listener) {
  const result = EventEmitter.prototype.prependOnceListener.call(this, name, listener);
  if (name === 'message') this.start();
  return result;
};

export function MessageChannel() {
  const ids = host.createMessageChannel();
  this.port1 = new MessagePort(ids.port1);
  this.port2 = new MessagePort(ids.port2);
}

export function Worker(filename, options) {
  EventEmitter.call(this);
  options = options || {};
  const normalized = normalizeWorkerOptions(options);
  const created = host.createWorker(String(filename), normalized);
  this.__id = unwrapWorkerId(created);
  this.threadId = unwrapWorkerThreadId(created);
  this.__closed = false;
  const worker = this;
  host.workerOnLifecycle(this.__id, {
    online: function () {
      if (!worker.__closed) worker.emit('online');
    },
    message: function (value) {
      if (!worker.__closed) worker.emit('message', value);
    },
    error: function (err) {
      if (!worker.__closed) worker.emit('error', err);
    },
    exit: function (code) {
      worker.__closed = true;
      worker.emit('exit', code);
    },
  });
}
Worker.prototype = Object.create(EventEmitter.prototype);
Worker.prototype.constructor = Worker;

Worker.prototype.postMessage = function (value, transferList) {
  if (this.__closed) return;
  host.workerPostMessage(this.__id, value, transferListOf(transferList));
};

Worker.prototype.terminate = function () {
  return host.workerTerminate(this.__id);
};

Worker.prototype.ref = function () {
  host.workerRef(this.__id);
  return this;
};

Worker.prototype.unref = function () {
  host.workerUnref(this.__id);
  return this;
};

export function receiveMessageOnPort(port) {
  const id = port && port.__id !== undefined ? port.__id : port;
  const msg = host.receiveMessageOnPort(id);
  if (msg === undefined || msg === null) return undefined;
  if (typeof msg === 'object' && Object.prototype.hasOwnProperty.call(msg, 'message')) return msg;
  return { message: msg };
}

export const isMainThread = host.getIsMainThread();
export const threadId = host.getThreadId();
export const workerData = host.getWorkerData();

function resolveParentPort() {
  if (isMainThread) return null;
  const id = host.getParentPortId();
  if (id === undefined || id === null) return null;
  return new MessagePort(id);
}
export const parentPort = resolveParentPort();

export const resourceLimits = {};
export const SHARE_ENV = Symbol.for('nodejs.worker_threads.SHARE_ENV');

const workerThreads = {
  isMainThread: isMainThread,
  parentPort: parentPort,
  threadId: threadId,
  workerData: workerData,
  Worker: Worker,
  MessageChannel: MessageChannel,
  MessagePort: MessagePort,
  receiveMessageOnPort: receiveMessageOnPort,
  resourceLimits: resourceLimits,
  SHARE_ENV: SHARE_ENV,
};
export default workerThreads;
