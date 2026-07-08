import { EventEmitter } from 'events';
import { Duplex } from 'stream';

function getHost() {
  const host = globalThis.__wjsm_node_tls;
  if (!host) throw new Error('wjsm internal tls host bridge is not installed');
  return host;
}
const host = getHost();

function schedule(fn) {
  if (typeof queueMicrotask === 'function') queueMicrotask(fn);
  else setTimeout(fn, 0);
}

function normalizeConnectArgs(a, b, c) {
  const out = { port: undefined, host: '127.0.0.1', options: {}, callback: undefined };
  if (typeof a === 'object' && a !== null && !Array.isArray(a)) {
    out.port = a.port;
    out.host = a.host || a.hostname || '127.0.0.1';
    out.options = a;
    if (typeof b === 'function') out.callback = b;
  } else {
    out.port = a;
    if (typeof b === 'object' && b !== null) { out.options = b; if (b.host) out.host = b.host; }
    else if (typeof b === 'string') out.host = b;
    if (typeof c === 'function') out.callback = c;
    else if (typeof b === 'function') out.callback = b;
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

export function TLSSocket(options) {
  Duplex.call(this, options || {});
  options = options || {};
  this.__handle = options.handle;
  this.connecting = false;
  this.pending = this.__handle === undefined;
  this.destroyed = false;
  this.readyState = this.__handle === undefined ? 'closed' : 'open';
  this.allowHalfOpen = Boolean(options.allowHalfOpen);
  this.__readClosed = false;
  this.alpnProtocol = undefined;
  this.servername = undefined;
  if (this.__handle !== undefined) {
    this.localAddress = host.socketLocalAddress(this.__handle);
    this.localPort = host.socketLocalPort(this.__handle);
    this.remoteAddress = host.socketRemoteAddress(this.__handle);
    this.remotePort = host.socketRemotePort(this.__handle);
  }
}

TLSSocket.prototype = Object.create(Duplex.prototype);
TLSSocket.prototype.constructor = TLSSocket;

TLSSocket.prototype.connect = function (a, b, c) {
  const opts = normalizeConnectArgs(a, b, c);
  const options = opts.options || {};
  if (opts.callback) this.once('secureConnect', opts.callback);
  this.connecting = true;
  this.pending = true;
  this.readyState = 'opening';
  this.servername = options.servername || opts.host;
  const self = this;
  const rejectUnauthorized = options.rejectUnauthorized === undefined ? true : Boolean(options.rejectUnauthorized);
  const alpnProtocols = options.ALPNProtocols
    ? (Array.isArray(options.ALPNProtocols) ? options.ALPNProtocols.join(',') : String(options.ALPNProtocols))
    : '';
  host.connect(opts.port, opts.host, this.servername, rejectUnauthorized, alpnProtocols).then(function (handle) {
    self.__handle = handle;
    self.connecting = false;
    self.pending = false;
    self.readyState = 'open';
    self.localAddress = host.socketLocalAddress(handle);
    self.localPort = host.socketLocalPort(handle);
    self.remoteAddress = host.socketRemoteAddress(handle);
    self.remotePort = host.socketRemotePort(handle);
    self.emit('secureConnect');
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

TLSSocket.prototype._startReadLoop = function () {
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

TLSSocket.prototype.write = function (chunk, encoding, callback) {
  if (typeof encoding === 'function') { callback = encoding; encoding = undefined; }
  if (this.__handle === undefined) throw new Error('TLSSocket is not connected');
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

TLSSocket.prototype.end = function (chunk, encoding, callback) {
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
      this.emit('error', result);
      this.destroy();
    } else finish();
  } else finish();
  return this;
};

TLSSocket.prototype.destroy = function (error) {
  if (this.destroyed) return this;
  this.destroyed = true;
  this.__readClosed = true;
  this.readyState = 'closed';
  if (this.__handle !== undefined) host.destroy(this.__handle);
  if (error) this.emit('error', error);
  this.emit('close', Boolean(error));
  return this;
};

TLSSocket.prototype.address = function () {
  return makeAddress(this.localAddress, this.localPort);
};
TLSSocket.prototype.setTimeout = function (msecs, callback) {
  if (callback) this.once('timeout', callback);
  return this;
};
TLSSocket.prototype.setNoDelay = function () { return this; };
TLSSocket.prototype.setKeepAlive = function () { return this; };

export function Server(options, connectionListener) {
  EventEmitter.call(this);
  if (typeof options === 'function') { connectionListener = options; options = {}; }
  this._options = options || {};
  this.__handle = undefined;
  this.__listening = false;
  this.__closing = false;
  this.maxConnections = undefined;
  this.connections = 0;
  if (connectionListener) this._events.secureConnection = [connectionListener];
}

Server.prototype = Object.create(EventEmitter.prototype);
Server.prototype.constructor = Server;

Server.prototype.listen = function (a, b, c) {
  const opts = normalizeListenArgs(a, b, c);
  if (opts.callback) this.once('listening', opts.callback);
  const self = this;
  const o = this._options;
  const certPem = o.cert ? (typeof o.cert === 'string' ? o.cert : '') : '';
  const keyPem = o.key ? (typeof o.key === 'string' ? o.key : '') : '';
  const alpnProtocols = o.ALPNProtocols
    ? (Array.isArray(o.ALPNProtocols) ? o.ALPNProtocols.join(',') : String(o.ALPNProtocols))
    : '';
  host.serverListen(opts.port, opts.host, certPem, keyPem, alpnProtocols).then(function (handle) {
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
    const socket = new TLSSocket({ handle: socketHandle });
    self.connections = self.connections + 1;
    socket.once('close', function () { self.connections = Math.max(0, self.connections - 1); });
    self.emit('secureConnection', socket);
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
Server.prototype.ref = function () { return this; };
Server.prototype.unref = function () { return this; };

export function createServer(options, connectionListener) {
  return new Server(options, connectionListener);
}

export function connect(a, b, c, d) {
  let port, host_, options, callback;
  if (typeof a === 'object' && a !== null) {
    options = a; port = a.port; host_ = a.host || '127.0.0.1';
    if (typeof b === 'function') callback = b;
  } else {
    port = a;
    if (typeof b === 'object' && b !== null) { options = b; if (b.host) host_ = b.host; else host_ = '127.0.0.1'; }
    else if (typeof b === 'string') { host_ = b; options = {}; }
    else { host_ = '127.0.0.1'; options = {}; }
    if (typeof c === 'function') callback = c;
    else if (typeof c === 'object' && c !== null) { options = c; if (c.host) host_ = c.host; }
    if (typeof d === 'function') callback = d;
  }
  const socket = new TLSSocket(options || {});
  if (callback) socket.once('secureConnect', callback);
  return socket.connect(port, host_, options || {});
}

export const DEFAULT_ECDH_CURVE = 'prime256v1';
export const rootCertificates = [];

const tls = {
  TLSSocket: TLSSocket,
  Server: Server,
  createServer: createServer,
  connect: connect,
  DEFAULT_ECDH_CURVE: DEFAULT_ECDH_CURVE,
  rootCertificates: rootCertificates,
};
export default tls;
