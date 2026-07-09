import { EventEmitter } from 'events';
import { Readable, Writable, Duplex, Transform, PassThrough, pipeline, finished } from 'stream';

function schedule(fn) {
  if (typeof queueMicrotask === 'function') queueMicrotask(fn);
  else setTimeout(fn, 0);
}

function asString(value) {
  if (value && typeof value.href === 'string') return value.href;
  return String(value);
}

function mergeOptions(input, options) {
  const out = { url: asString(input), method: 'GET', headers: {}, body: undefined };
  if (options && typeof options === 'object') {
    if (options.href) out.url = String(options.href);
    if (options.protocol || options.hostname || options.host || options.path) {
      const protocol = options.protocol || 'http:';
      const host = options.hostname || options.host || 'localhost';
      const port = options.port ? ':' + options.port : '';
      const path = options.path || '/';
      out.url = protocol + '//' + host + port + path;
    }
    if (options.method) out.method = String(options.method).toUpperCase();
    if (options.headers) out.headers = options.headers;
  }
  return out;
}

function headersToObject(headers) {
  const out = {};
  if (!headers) return out;
  if (typeof headers.forEach === 'function') {
    headers.forEach(function (value, name) { out[String(name).toLowerCase()] = String(value); });
    return out;
  }
  const keys = Object.keys(headers);
  for (var i = 0; i < keys.length; i = i + 1) out[keys[i].toLowerCase()] = String(headers[keys[i]]);
  return out;
}

function parseHeaders(lines) {
  const headers = {};
  for (var i = 1; i < lines.length; i = i + 1) {
    const line = lines[i];
    const colon = line.indexOf(':');
    if (colon > 0) headers[line.slice(0, colon).toLowerCase()] = line.slice(colon + 1).trim();
  }
  return headers;
}

function statusText(code) {
  const known = { '200': 'OK', '201': 'Created', '204': 'No Content', '301': 'Moved Permanently', '302': 'Found', '400': 'Bad Request', '404': 'Not Found', '500': 'Internal Server Error' };
  return known[String(code)] || 'OK';
}

export function IncomingMessage(statusCode, headers, body, method, url) {
  EventEmitter.call(this);
  this.statusCode = statusCode || 0;
  this.statusMessage = '';
  this.headers = headers || {};
  this.method = method;
  this.url = url;
  this.readable = true;
  this.readableEnded = false;
  this.destroyed = false;
  this._readableBuffer = [];
  this._readableFlowing = false;
  this._readablePipes = [];
  const self = this;
  schedule(function () {
    if (body !== undefined && body !== null && body.length !== 0) self._readableBuffer.push(body);
    self.readableEnded = true;
    if (self._readableFlowing) emitBuffered(self);
  });
}

function emitBuffered(stream) {
  while (stream._readableFlowing && stream._readableBuffer.length > 0) {
    stream.emit('data', stream._readableBuffer.shift());
  }
  if (stream._readableFlowing && stream.readableEnded) {
    stream.emit('end');
  }
}

IncomingMessage.prototype = Object.create(EventEmitter.prototype);
IncomingMessage.prototype.constructor = IncomingMessage;
IncomingMessage.prototype.on = function (name, listener) {
  const result = EventEmitter.prototype.on.call(this, name, listener);
  if (name === 'data') {
    this._readableFlowing = true;
    const self = this;
    schedule(function () { emitBuffered(self); });
  }
  return result;
};
IncomingMessage.prototype.addListener = IncomingMessage.prototype.on;

export function ClientRequest(input, options, callback) {
  EventEmitter.call(this);
  this._requestOptions = mergeOptions(input, options);
  this._chunks = [];
  this.aborted = false;
  this.finished = false;
  this.writable = true;
  this.writableEnded = false;
  if (typeof options === 'function') callback = options;
  if (callback) this.once('response', callback);
}

ClientRequest.prototype = Object.create(EventEmitter.prototype);
ClientRequest.prototype.constructor = ClientRequest;
ClientRequest.prototype.write = function (chunk) { this._chunks.push(chunk); return true; };
ClientRequest.prototype.end = function (chunk, encoding, callback) {
  if (typeof chunk === 'function') { callback = chunk; chunk = undefined; }
  if (chunk !== undefined) this.write(chunk);
  this.finished = true;
  if (callback) callback();
  const self = this;
  const bodyChunks = [];
  for (var i = 0; i < self._chunks.length; i = i + 1) bodyChunks.push(Buffer.from(self._chunks[i]));
  const body = bodyChunks.length === 0 ? undefined : Buffer.concat(bodyChunks);
  fetch(self._requestOptions.url, {
    method: self._requestOptions.method,
    headers: self._requestOptions.headers,
    body: body
  }).then(function (response) {
    return response.arrayBuffer().then(function (buffer) {
      const msg = new IncomingMessage(
        response.status,
        headersToObject(response.headers),
        Buffer.from(buffer),
        self._requestOptions.method,
        self._requestOptions.url
      );
      self.emit('response', msg);
      self.emit('finish');
      self.emit('close');
    });
  }, function (error) {
    self.emit('error', error);
    self.emit('close');
  });
  return self;
};
ClientRequest.prototype.abort = function () { this.aborted = true; this.emit('abort'); this.emit('close'); };
ClientRequest.prototype.destroy = function (error) { if (error) this.emit('error', error); this.abort(); return this; };

export function request(input, options, callback) {
  return new ClientRequest(input, options, callback);
}

export function get(input, options, callback) {
  const req = request(input, options, callback);
  req.end();
  return req;
}

export function Server(requestListener) {
  EventEmitter.call(this);
  this._netHandle = undefined;
  this._address = undefined;
  if (typeof requestListener === 'function') this._events.request = [requestListener];
}

Server.prototype = Object.create(EventEmitter.prototype);
Server.prototype.constructor = Server;
function httpOnBound(server, handle) {
  const netHost = globalThis.__wjsm_node_net;
  server._netHandle = handle;
  server._address = {
    address: netHost.serverAddress(handle),
    family: 'IPv4',
    port: netHost.serverPort(handle),
  };
  server.__listening = true;
  server.emit('listening');
  httpAcceptLoop(server);
}

function httpAcceptLoop(server) {
  if (!server.__listening || server._netHandle === undefined) return;
  const netHost = globalThis.__wjsm_node_net;
  const handle = server._netHandle;
  netHost.serverAccept(handle).then(function (socketHandle) {
    if (socketHandle === null || socketHandle === undefined || !server.__listening) return;
    // 最小 socket 适配
    const sock = { __handle: socketHandle };
    sock.on = function (ev, fn) {
      if (ev === 'data') {
        function readLoop() {
          netHost.read(socketHandle).then(function (buf) {
            if (buf === null || buf === undefined) return;
            fn(Buffer.from(buf));
            schedule(readLoop);
          });
        }
        schedule(readLoop);
      }
      return sock;
    };
    sock.end = function (data, cb) {
      if (data !== undefined) netHost.write(socketHandle, data);
      netHost.end(socketHandle);
      if (cb) schedule(cb);
      return sock;
    };
    handleHttpConnection(server, sock);
    schedule(function () { httpAcceptLoop(server); });
  });
}

Server.prototype.listen = function (a, b, c) {
  const port = a === undefined ? 0 : a;
  let hostName = '127.0.0.1';
  let callback = undefined;
  if (typeof b === 'function') callback = b;
  else if (b !== undefined) hostName = b;
  if (typeof c === 'function') callback = c;
  if (callback) this.once('listening', callback);
  const server = this;
  const netHost = globalThis.__wjsm_node_net;
  const p = Number(port) || 0;
  const h = String(hostName || '127.0.0.1');
  netHost.serverListen(p, h).then(function (handle) {
    httpOnBound(server, handle);
  });
  return this;
};

Server.prototype.close = function (callback) {
  if (callback) this.once('close', callback);
  const self = this;
  self.__listening = false;
  if (self._netHandle !== undefined) {
    globalThis.__wjsm_node_net.serverClose(self._netHandle);
  }
  self._netHandle = undefined;
  schedule(function () { self.emit('close'); });
  return this;
};
Server.prototype.address = function () { return this._address; };

export function ServerResponse(socket) {
  EventEmitter.call(this);
  this.socket = socket;
  this.statusCode = 200;
  this.statusMessage = 'OK';
  this.headersSent = false;
  this.writableEnded = false;
  this._headers = {};
  this._chunks = [];
}

ServerResponse.prototype = Object.create(EventEmitter.prototype);
ServerResponse.prototype.constructor = ServerResponse;
ServerResponse.prototype.setHeader = function (name, value) { this._headers[String(name).toLowerCase()] = String(value); };
ServerResponse.prototype.getHeader = function (name) { return this._headers[String(name).toLowerCase()]; };
ServerResponse.prototype.writeHead = function (statusCode, statusMessage, headers) {
  this.statusCode = statusCode;
  if (typeof statusMessage === 'object') { headers = statusMessage; statusMessage = undefined; }
  if (statusMessage !== undefined) this.statusMessage = String(statusMessage);
  const keys = headers ? Object.keys(headers) : [];
  for (var i = 0; i < keys.length; i = i + 1) this.setHeader(keys[i], headers[keys[i]]);
  return this;
};
ServerResponse.prototype.write = function (chunk, encoding, callback) {
  this._chunks.push(Buffer.from(chunk === undefined ? '' : chunk, encoding || 'utf8'));
  if (callback) callback();
  return true;
};
ServerResponse.prototype.end = function (chunk, encoding, callback) {
  if (typeof chunk === 'function') { callback = chunk; chunk = undefined; encoding = undefined; }
  else if (typeof encoding === 'function') { callback = encoding; encoding = undefined; }
  if (chunk !== undefined) this.write(chunk, encoding);
  const body = this._chunks.length === 0 ? Buffer.from('') : Buffer.concat(this._chunks);
  if (this.getHeader('content-length') === undefined) this.setHeader('content-length', body.length);
  if (this.getHeader('connection') === undefined) this.setHeader('connection', 'close');
  const lines = ['HTTP/1.1 ' + this.statusCode + ' ' + (this.statusMessage || statusText(this.statusCode))];
  const keys = Object.keys(this._headers);
  for (var i = 0; i < keys.length; i = i + 1) lines.push(keys[i] + ': ' + this._headers[keys[i]]);
  const payload = Buffer.concat([Buffer.from(lines.join('\r\n') + '\r\n\r\n'), body]);
  this.headersSent = true;
  this.writableEnded = true;
  const self = this;
  this.socket.end(payload, function () {
    self.emit('finish');
    self.emit('close');
    if (callback) callback();
  });
  return this;
};

function handleHttpConnection(server, socket) {
  const chunks = [];
  socket.on('data', function (chunk) {
    chunks.push(Buffer.from(chunk));
    const raw = Buffer.concat(chunks).toString();
    const headerEnd = raw.indexOf('\r\n\r\n');
    if (headerEnd < 0) return;
    const head = raw.slice(0, headerEnd);
    const lines = head.split('\r\n');
    const requestLine = lines[0].split(' ');
    const method = requestLine[0] || 'GET';
    const url = requestLine[1] || '/';
    const body = Buffer.from(raw.slice(headerEnd + 4));
    const req = new IncomingMessage(0, parseHeaders(lines), body, method, url);
    const res = new ServerResponse(socket);
    server.emit('request', req, res);
  });
  socket.on('error', function (error) { server.emit('clientError', error, socket); });
}

export function createServer(requestListener) {
  return new Server(requestListener);
}

export const METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS'];
export const STATUS_CODES = { '200': 'OK', '201': 'Created', '204': 'No Content', '301': 'Moved Permanently', '302': 'Found', '400': 'Bad Request', '404': 'Not Found', '500': 'Internal Server Error' };
export function Agent(options) { this.options = options || {}; }
export const globalAgent = new Agent();

const http = { request: request, get: get, ClientRequest: ClientRequest, IncomingMessage: IncomingMessage, Server: Server, createServer: createServer, METHODS: METHODS, STATUS_CODES: STATUS_CODES, Agent: Agent, globalAgent: globalAgent };
export default http;
