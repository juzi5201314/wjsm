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
  const baseOn = this.on;
  this.on = function (name, listener) {
    const result = baseOn.call(self, name, listener);
    if (name === 'data') { self._readableFlowing = true; schedule(() => emitBuffered(self)); }
    return result;
  };
  this.addListener = this.on;
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

export function ClientRequest(input, options, callback) {
  EventEmitter.call(this);
  this._requestOptions = mergeOptions(input, options);
  this._chunks = [];
  this.aborted = false;
  this.finished = false;
  this.writable = true;
  this.writableEnded = false;
  const self = this;
  if (typeof options === 'function') callback = options;
  if (callback) this.once('response', callback);
  this.write = function (chunk) { self._chunks.push(chunk); return true; };
  this.end = function (chunk, encoding, callback) {
    if (typeof chunk === 'function') { callback = chunk; chunk = undefined; }
    if (chunk !== undefined) self.write(chunk);
    self.finished = true;
    if (callback) callback();
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
  this.abort = function () { self.aborted = true; self.emit('abort'); self.emit('close'); };
  this.destroy = function (error) { if (error) self.emit('error', error); self.abort(); return self; };
}

export function request(input, options, callback) {
  return new ClientRequest(input, options, callback);
}

export function get(input, options, callback) {
  const req = request(input, options, callback);
  req.end();
  return req;
}

export function Server() {
  EventEmitter.call(this);
}

export function createServer() {
  const s = {};
  EventEmitter.call(s);
  s.listen = function () { throw new Error('http.Server requires net/TCP support tracked by issue #313'); };
  s.close = function (callback) { if (callback) callback(); s.emit('close'); };
  return s;
}

export const METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS'];
export const STATUS_CODES = { '200': 'OK', '201': 'Created', '204': 'No Content', '301': 'Moved Permanently', '302': 'Found', '400': 'Bad Request', '404': 'Not Found', '500': 'Internal Server Error' };
export function Agent(options) { this.options = options || {}; }
export const globalAgent = new Agent();

const http = { request: request, get: get, ClientRequest: ClientRequest, IncomingMessage: IncomingMessage, Server: Server, createServer: createServer, METHODS: METHODS, STATUS_CODES: STATUS_CODES, Agent: Agent, globalAgent: globalAgent };
export default http;
