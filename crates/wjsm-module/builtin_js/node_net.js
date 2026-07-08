import { EventEmitter } from 'events';
import { Duplex } from 'stream';

function getHost() {
  const host = globalThis.__wjsm_node_net;
  if (!host) throw new Error('wjsm internal net host bridge is not installed');
  return host;
}
const host = getHost();

function schedule(fn) {
  if (typeof queueMicrotask === 'function') queueMicrotask(fn);
  else setTimeout(fn, 0);
}

function normalizeConnectArgs(a, b, c) {
  const out = { port: undefined, host: '127.0.0.1', callback: undefined };
  if (typeof a === 'object' && a !== null) {
    out.port = a.port;
    out.host = a.host || a.hostname || '127.0.0.1';
    if (typeof b === 'function') out.callback = b;
  } else {
    out.port = a;
    if (typeof b === 'function') out.callback = b;
    else if (b !== undefined) out.host = b;
    if (typeof c === 'function') out.callback = c;
  }
  return out;
}

function normalizeListenArgs(a, b, c) {
  const out = { port: 0, host: '127.0.0.1', callback: undefined };
  if (typeof a === 'object' && a !== null) {
    out.port = a.port || 0;
    out.host = a.host || a.hostname || '127.0.0.1';
    if (typeof b === 'function') out.callback = b;
  } else {
    if (a !== undefined) out.port = a;
    if (typeof b === 'function') out.callback = b;
    else if (b !== undefined) out.host = b;
    if (typeof c === 'function') out.callback = c;
  }
  return out;
}

function makeAddress(address, port) {
  return { address: address || '127.0.0.1', family: address && address.indexOf(':') >= 0 ? 'IPv6' : 'IPv4', port: port || 0 };
}

export function Socket(options) {
  Duplex.call(this, options || {});
  options = options || {};
  this.__handle = options.handle;
  this.connecting = false;
  this.pending = false;
  this.destroyed = false;
  this.readyState = this.__handle === undefined ? 'closed' : 'open';
  this.allowHalfOpen = Boolean(options.allowHalfOpen);
  this.__readClosed = false;
  if (this.__handle !== undefined) {
    this.localAddress = host.socketLocalAddress(this.__handle);
    this.localPort = host.socketLocalPort(this.__handle);
    this.remoteAddress = host.socketRemoteAddress(this.__handle);
    this.remotePort = host.socketRemotePort(this.__handle);
  }
}

Socket.prototype = Object.create(Duplex.prototype);
Socket.prototype.constructor = Socket;

Socket.prototype.connect = function (a, b, c) {
  const opts = normalizeConnectArgs(a, b, c);
  if (opts.callback) this.once('connect', opts.callback);
  this.connecting = true;
  this.pending = true;
  this.readyState = 'opening';
  const self = this;
  host.connect(opts.port, opts.host).then(function (handle) {
    self.__handle = handle;
    self.connecting = false;
    self.pending = false;
    self.readyState = 'open';
    self.localAddress = host.socketLocalAddress(handle);
    self.localPort = host.socketLocalPort(handle);
    self.remoteAddress = host.socketRemoteAddress(handle);
    self.remotePort = host.socketRemotePort(handle);
    self.emit('connect');
    self.emit('ready');
    self._startReadLoop();
  }, function (error) {
    self.connecting = false;
    self.pending = false;
    self.readyState = 'closed';
    self.destroyed = true;
    self.emit('error', error);
    self.emit('close', true);
  });
  return this;
};

Socket.prototype._startReadLoop = function () {
  if (this.__reading || this.__handle === undefined || this.destroyed || this.__readClosed) return;
  this.__reading = true;
  const self = this;
  function pump() {
    if (self.destroyed || self.__readClosed || self.__handle === undefined) { self.__reading = false; return; }
    host.read(self.__handle).then(function (chunk) {
      if (self.destroyed) { self.__reading = false; return; }
      if (chunk === null) {
        self.readableEnded = true;
        self.readyState = self.writableEnded ? 'closed' : 'readOnly';
        self.push(null);
        self.emit('end');
        if (!self.allowHalfOpen) self.end();
        self.emit('close', false);
        self.__reading = false;
        return;
      }
      self.push(chunk);
      self.emit('data', chunk);
      schedule(pump);
    }, function (error) {
      self.destroyed = true;
      self.readyState = 'closed';
      self.emit('error', error);
      self.emit('close', true);
    });
  }
  pump();
};

Socket.prototype.write = function (chunk, encoding, callback) {
  if (typeof encoding === 'function') { callback = encoding; encoding = undefined; }
  if (this.__handle === undefined) throw new Error('Socket is not connected');
  const data = typeof chunk === 'string' ? chunk : chunk.toString();
  const result = host.write(this.__handle, data);
  if (result instanceof Error) {
    if (callback) callback(result);
    this.emit('error', result);
    this.destroy();
  } else {
    if (callback) callback();
    this.emit('drain');
  }
  return true;
};

Socket.prototype.end = function (chunk, encoding, callback) {
  if (typeof chunk === 'function') { callback = chunk; chunk = undefined; encoding = undefined; }
  else if (typeof encoding === 'function') { callback = encoding; encoding = undefined; }
  if (this.__handle === undefined || this.writableEnded) {
    if (callback) callback();
    return this;
  }
  this.writableEnded = true;
  this.__readClosed = true;
  this.readyState = this.readableEnded ? 'closed' : 'writeOnly';
  const self = this;
  const finish = function () {
    const result = host.end(self.__handle);
    if (result instanceof Error) {
      if (callback) callback(result);
      self.emit('error', result);
      self.destroy();
    } else {
      self.writableFinished = true;
      self.emit('finish');
      if (callback) callback();
    }
  };
  if (chunk !== undefined) {
    const data = typeof chunk === 'string' ? chunk : chunk.toString();
    const result = host.write(this.__handle, data);
    if (result instanceof Error) {
      if (callback) callback(result);
      self.emit('error', result);
      self.destroy();
    } else finish();
  } else finish();
  return this;
};

Socket.prototype.destroy = function (error) {
  if (this.destroyed) return this;
  this.destroyed = true;
  this.__readClosed = true;
  this.readyState = 'closed';
  if (this.__handle !== undefined) host.destroy(this.__handle);
  if (error) this.emit('error', error);
  this.emit('close', Boolean(error));
  return this;
};

Socket.prototype.setTimeout = function (msecs, callback) {
  if (callback) this.once('timeout', callback);
  return this;
};
Socket.prototype.setNoDelay = function () { return this; };
Socket.prototype.setKeepAlive = function () { return this; };
Socket.prototype.address = function () { return makeAddress(this.localAddress, this.localPort); };

export function Server(options, connectionListener) {
  EventEmitter.call(this);
  if (typeof options === 'function') { connectionListener = options; options = {}; }
  this.__handle = undefined;
  this.__listening = false;
  this.__closing = false;
  this.maxConnections = undefined;
  this.connections = 0;
  if (connectionListener) this._events.connection = [connectionListener];
}

Server.prototype = Object.create(EventEmitter.prototype);
Server.prototype.constructor = Server;

Server.prototype.listen = function (a, b, c) {
  const opts = normalizeListenArgs(a, b, c);
  if (opts.callback) this.once('listening', opts.callback);
  const self = this;
  host.serverListen(opts.port, opts.host).then(function (handle) {
    self.__handle = handle;
    self.__listening = true;
    self.__closing = false;
    self.emit('listening');
    self.__acceptLoop();
  }, function (error) {
    self.emit('error', error);
  });
  return this;
};

Server.prototype.__acceptLoop = function () {
  if (!this.__listening || this.__closing || this.__handle === undefined) return;
  const self = this;
  host.serverAccept(this.__handle).then(function (socketHandle) {
    if (socketHandle === null || self.__closing) return;
    const socket = new Socket({ handle: socketHandle });
    self.connections = self.connections + 1;
    socket.once('close', function () { self.connections = Math.max(0, self.connections - 1); });
    self.emit('connection', socket);
    socket._startReadLoop();
    schedule(function () { self.__acceptLoop(); });
  }, function (error) {
    if (!self.__closing) self.emit('error', error);
  });
};

Server.prototype.close = function (callback) {
  if (callback) this.once('close', callback);
  if (this.__handle === undefined || this.__closing) {
    const self = this;
    schedule(function () { self.emit('close'); });
    return this;
  }
  this.__closing = true;
  this.__listening = false;
  const handle = this.__handle;
  this.__handle = undefined;
  host.serverClose(handle);
  const self = this;
  schedule(function () { self.emit('close'); });
  return this;
};

Server.prototype.address = function () {
  if (this.__handle === undefined) return null;
  return makeAddress(host.serverAddress(this.__handle), host.serverPort(this.__handle));
};
Server.prototype.getConnections = function (callback) { callback(null, this.connections); };
Server.prototype.ref = function () { return this; };
Server.prototype.unref = function () { return this; };

export function createServer(options, connectionListener) {
  return new Server(options, connectionListener);
}

export function createConnection(a, b, c) {
  return new Socket().connect(a, b, c);
}

export const connect = createConnection;
export const Stream = Socket;
export const isIP = function (input) {
  const value = String(input);
  if (value.indexOf(':') >= 0) return 6;
  const parts = value.split('.');
  if (parts.length !== 4) return 0;
  for (let i = 0; i < parts.length; i = i + 1) {
    const n = Number(parts[i]);
    if (!isFinite(n) || n < 0 || n > 255) return 0;
  }
  return 4;
};
export const isIPv4 = function (input) { return isIP(input) === 4; };
export const isIPv6 = function (input) { return isIP(input) === 6; };

const net = { Socket: Socket, Server: Server, Stream: Stream, createServer: createServer, createConnection: createConnection, connect: connect, isIP: isIP, isIPv4: isIPv4, isIPv6: isIPv6 };
export default net;
