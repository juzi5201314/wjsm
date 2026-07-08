import { EventEmitter } from 'events';

function getHost() {
  const host = globalThis.__wjsm_node_dgram;
  if (!host) throw new Error('wjsm internal dgram host bridge is not installed');
  return host;
}
const host = getHost();
const hostBind = host.bind;

function schedule(fn) {
  if (typeof queueMicrotask === 'function') queueMicrotask(fn);
  else setTimeout(fn, 0);
}

function makeAddress(address, port) {
  return { address: address || '127.0.0.1', family: address && address.indexOf(':') >= 0 ? 'IPv6' : 'IPv4', port: port || 0 };
}

export function Socket(type, callback) {
  EventEmitter.call(this);
  this.type = type || 'udp4';
  this.__handle = undefined;
  this.closed = false;
  this.bound = false;
  this.__recvPending = false;
  if (callback) this.on('message', callback);
}

Socket.prototype = Object.create(EventEmitter.prototype);
Socket.prototype.constructor = Socket;

Socket.prototype.bind = function (a, b, callback) {
  const opts = typeof a === 'object' && a !== null
    ? { port: a.port || 0, host: a.host || a.address || '127.0.0.1', callback: typeof b === 'function' ? b : undefined }
    : { port: a === undefined ? 0 : a, host: typeof b === 'string' ? b : '127.0.0.1', callback: typeof callback === 'function' ? callback : (typeof b === 'function' ? b : undefined) };
  if (opts.callback) this.once('listening', opts.callback);
  const self = this;
  hostBind(opts.port, opts.host).then(function (handle) {
    self.__handle = handle;
    self.bound = true;
    self.localAddress = host.address(handle);
    self.localPort = host.port(handle);
    self.emit('listening');
    self.__recvLoop();
  }, function (error) {
    self.emit('error', error);
  });
  return this;
};

Socket.prototype.__recvLoop = function () {
  if (this.closed || this.__handle === undefined || this.__recvPending) return;
  this.__recvPending = true;
  const self = this;
  host.recv(this.__handle).then(function (result) {
    self.__recvPending = false;
    if (self.closed) return;
    if (result === null) {
      self.emit('close');
      return;
    }
    const msg = Buffer.from(result.data);
    const rinfo = { address: result.address, port: result.port, family: result.address && result.address.indexOf(':') >= 0 ? 'IPv6' : 'IPv4', size: msg.length };
    self.emit('message', msg, rinfo);
    schedule(function () { self.__recvLoop(); });
  }, function (error) {
    self.__recvPending = false;
    if (!self.closed) self.emit('error', error);
  });
};

Socket.prototype.send = function (msg, offset, length, port, host_, callback) {
  if (typeof offset === 'function') { callback = offset; offset = 0; length = undefined; }
  else if (typeof length === 'function') { callback = length; length = undefined; }
  else if (typeof port === 'function') { callback = port; port = undefined; }
  else if (typeof host_ === 'function') { callback = host_; host_ = undefined; }
  if (this.__handle === undefined) {
    const self = this;
    this['bind'](0, '127.0.0.1', function () {
      self.send(msg, offset, length, port, host_, callback);
    });
    return true;
  }
  offset = offset || 0;
  const data = typeof msg === 'string' ? Buffer.from(msg) : Buffer.from(msg);
  const slice = length !== undefined ? data.slice(offset, offset + length) : data.slice(offset);
  const portVal = port || 0;
  const hostVal = host_ || '127.0.0.1';
  const result = host.send(this.__handle, slice, portVal, hostVal);
  if (result instanceof Error) {
    if (callback) callback(result);
    else this.emit('error', result);
    return false;
  }
  if (callback) callback();
  return true;
};

Socket.prototype.close = function (callback) {
  if (callback) this.once('close', callback);
  if (this.closed) return this;
  this.closed = true;
  if (this.__handle !== undefined) {
    host.close(this.__handle);
    this.__handle = undefined;
  }
  const self = this;
  schedule(function () { self.emit('close'); });
  return this;
};

Socket.prototype.address = function () {
  if (this.__handle === undefined) return null;
  return makeAddress(host.address(this.__handle), host.port(this.__handle));
};

Socket.prototype.ref = function () { return this; };
Socket.prototype.unref = function () { return this; };
Socket.prototype.setBroadcast = function () { return this; };
Socket.prototype.setTTL = function () { return this; };
Socket.prototype.setMulticastTTL = function () { return this; };
Socket.prototype.setMulticastLoopback = function () { return this; };
Socket.prototype.addMembership = function () { return this; };
Socket.prototype.dropMembership = function () { return this; };
Socket.prototype.setSendBufferSize = function () { return this; };
Socket.prototype.setRecvBufferSize = function () { return this; };

export function createSocket(type, callback) {
  return new Socket(type, callback);
}

const dgram = { createSocket: createSocket, Socket: Socket };
export default dgram;
