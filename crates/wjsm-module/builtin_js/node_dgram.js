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
  return {
    address: address || '127.0.0.1',
    family: address && address.indexOf(':') >= 0 ? 'IPv6' : 'IPv4',
    port: port || 0,
  };
}

// handle → Socket。recv 回调只通过 handle 找回 socket，
// 避免方法体 const self 在并发调用间共享槽导致派发错位。
const socketsByHandle = Object.create(null);

// 为每个 handle 生成独立的 arm 函数对象，捕获该 handle 的专属闭包环境。
function makeArmRecv(handle) {
  return function armRecv() {
    const socket = socketsByHandle[handle];
    if (!socket || socket.closed || socket.__recvPending) return;
    socket.__recvPending = true;
    host.recv(handle).then(function (result) {
      const self = socketsByHandle[handle];
      if (!self) return;
      self.__recvPending = false;
      if (self.closed) return;
      if (result === null) {
        self.emit('close');
        return;
      }
      // host.recv 已返回 Buffer（Node 兼容）
      const msg = result.data;
      const rinfo = {
        address: result.address,
        port: result.port,
        family: result.address && result.address.indexOf(':') >= 0 ? 'IPv6' : 'IPv4',
        size: msg && msg.length !== undefined ? msg.length : 0,
      };
      self.emit('message', msg, rinfo);
      schedule(function () {
        const again = socketsByHandle[handle];
        if (again && again.__armRecv) again.__armRecv();
      });
    }, function (error) {
      const self = socketsByHandle[handle];
      if (!self) return;
      self.__recvPending = false;
      if (!self.closed) self.emit('error', error);
    });
  };
}

export function Socket(type, callback) {
  EventEmitter.call(this);
  this.type = type || 'udp4';
  this.__handle = undefined;
  this.closed = false;
  this.bound = false;
  this.__recvPending = false;
  this.__armRecv = undefined;
  if (callback) this.on('message', callback);
}

Socket.prototype = Object.create(EventEmitter.prototype);
Socket.prototype.constructor = Socket;

Socket.prototype.bind = function (a, b, callback) {
  const opts = typeof a === 'object' && a !== null
    ? {
        port: a.port || 0,
        host: a.host || a.address || '127.0.0.1',
        callback: typeof b === 'function' ? b : undefined,
      }
    : {
        port: a === undefined ? 0 : a,
        host: typeof b === 'string' ? b : '127.0.0.1',
        callback:
          typeof callback === 'function'
            ? callback
            : typeof b === 'function'
              ? b
              : undefined,
      };
  if (opts.callback) this.once('listening', opts.callback);
  const socket = this;
  hostBind(opts.port, opts.host).then(function (handle) {
    socket.__handle = handle;
    socket.bound = true;
    socket.localAddress = host.address(handle);
    socket.localPort = host.port(handle);
    socketsByHandle[handle] = socket;
    socket.__armRecv = makeArmRecv(handle);
    // 必须先启动 recv，再 emit('listening')：listening 回调里通常立刻 send
    socket.__armRecv();
    socket.emit('listening');
  }, function (error) {
    socket.emit('error', error);
  });
  return this;
};

Socket.prototype.__recvLoop = function () {
  if (this.__armRecv) this.__armRecv();
};

Socket.prototype.send = function (msg, offset, length, port, host_, callback) {
  // Node 兼容：
  //   send(msg, port[, address][, callback])
  //   send(msg, offset, length, port[, address][, callback])
  if (typeof offset === 'function') {
    callback = offset;
    offset = undefined;
    length = undefined;
    port = undefined;
    host_ = undefined;
  } else if (typeof length === 'function') {
    callback = length;
    port = offset;
    host_ = undefined;
    offset = undefined;
    length = undefined;
  } else if (typeof port === 'function') {
    if (typeof length === 'string') {
      callback = port;
      port = offset;
      host_ = length;
      offset = undefined;
      length = undefined;
    } else {
      callback = port;
      port = undefined;
      host_ = undefined;
    }
  } else if (typeof host_ === 'function') {
    callback = host_;
    host_ = undefined;
  } else if (
    typeof offset === 'number'
    && typeof length === 'string'
    && (port === undefined || typeof port === 'function')
  ) {
    callback = typeof port === 'function' ? port : callback;
    port = offset;
    host_ = length;
    offset = undefined;
    length = undefined;
  } else if (
    typeof offset === 'number'
    && (length === undefined || typeof length === 'string' || typeof length === 'function')
    && typeof length !== 'number'
  ) {
    if (typeof length === 'function') {
      callback = length;
      host_ = undefined;
    } else if (typeof length === 'string') {
      host_ = length;
    }
    port = offset;
    offset = undefined;
    length = undefined;
  }
  if (this.__handle === undefined) {
    const socket = this;
    this.bind(0, '127.0.0.1', function () {
      socket.send(msg, offset, length, port, host_, callback);
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
    const handle = this.__handle;
    delete socketsByHandle[handle];
    host.close(handle);
    this.__handle = undefined;
    this.__armRecv = undefined;
  }
  const socket = this;
  schedule(function () {
    socket.emit('close');
  });
  return this;
};

Socket.prototype.address = function () {
  if (this.__handle === undefined) return null;
  return makeAddress(this.localAddress, this.localPort);
};

Socket.prototype.ref = function () {
  return this;
};
Socket.prototype.unref = function () {
  return this;
};
Socket.prototype.setBroadcast = function () {
  return this;
};
Socket.prototype.setTTL = function () {
  return this;
};
Socket.prototype.setMulticastTTL = function () {
  return this;
};
Socket.prototype.setMulticastLoopback = function () {
  return this;
};
Socket.prototype.addMembership = function () {
  return this;
};
Socket.prototype.dropMembership = function () {
  return this;
};
Socket.prototype.setSendBufferSize = function () {
  return this;
};
Socket.prototype.setRecvBufferSize = function () {
  return this;
};

export function createSocket(type, callback) {
  return new Socket(type, callback);
}

const dgram = { createSocket: createSocket, Socket: Socket };
export default dgram;
