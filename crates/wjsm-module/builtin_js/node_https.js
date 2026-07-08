import { EventEmitter } from 'events';

function schedule(fn) {
  if (typeof queueMicrotask === 'function') queueMicrotask(fn);
  else setTimeout(fn, 0);
}

export function ClientRequest() {
  EventEmitter.call(this);
}
ClientRequest.prototype = Object.create(EventEmitter.prototype);
ClientRequest.prototype.constructor = ClientRequest;
ClientRequest.prototype.end = function () { this.emit('finish'); return this; };
ClientRequest.prototype.write = function () { return true; };
ClientRequest.prototype.abort = function () { this.emit('abort'); this.emit('close'); };
ClientRequest.prototype.destroy = function (error) { if (error) this.emit('error', error); this.abort(); return this; };

export function IncomingMessage() {
  EventEmitter.call(this);
}
IncomingMessage.prototype = Object.create(EventEmitter.prototype);
IncomingMessage.prototype.constructor = IncomingMessage;

export function Server(options, requestListener) {
  EventEmitter.call(this);
  if (typeof options === 'function') { requestListener = options; options = {}; }
  this._tlsOptions = options || {};
  this._tlsHandle = undefined;
  this._address = undefined;
  if (typeof requestListener === 'function') this._events.request = [requestListener];
}
Server.prototype = Object.create(EventEmitter.prototype);
Server.prototype.constructor = Server;
Server.prototype.listen = function (a, b, c) {
  const opts = { port: a === undefined ? 0 : a, host: '127.0.0.1', callback: undefined };
  if (typeof b === 'function') opts.callback = b;
  else if (b !== undefined) opts.host = b;
  if (typeof c === 'function') opts.callback = c;
  if (opts.callback) this.once('listening', opts.callback);
  const self = this;
  const host = globalThis.__wjsm_node_tls;
  const o = this._tlsOptions || {};
  const certPem = o.cert ? (typeof o.cert === 'string' ? o.cert : '') : '';
  const keyPem = o.key ? (typeof o.key === 'string' ? o.key : '') : '';
  const alpnProtocols = o.ALPNProtocols ? (Array.isArray(o.ALPNProtocols) ? o.ALPNProtocols.join(',') : String(o.ALPNProtocols)) : '';
  host.serverListen(opts.port, opts.host, certPem, keyPem, alpnProtocols).then(function (handle) {
    self._tlsHandle = handle;
    self._address = { address: host.serverAddress(handle), family: 'IPv4', port: host.serverPort(handle) };
    self.emit('listening');
  }, function (error) { self.emit('error', error); });
  return this;
};
Server.prototype.close = function (callback) {
  if (callback) this.once('close', callback);
  if (this._tlsHandle !== undefined) globalThis.__wjsm_node_tls.serverClose(this._tlsHandle);
  this._tlsHandle = undefined;
  const self = this;
  schedule(function () { self.emit('close'); });
  return this;
};
Server.prototype.address = function () { return this._address; };

export function request(input, options, callback) {
  const req = new ClientRequest();
  if (typeof options === 'function') callback = options;
  if (typeof callback === 'function') req.once('response', callback);
  return req;
}

export function get(input, options, callback) {
  const req = request(input, options, callback);
  req.end();
  return req;
}

export function createServer(options, requestListener) {
  return new Server(options, requestListener);
}

export const METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS'];
export const STATUS_CODES = { '200': 'OK', '201': 'Created', '204': 'No Content', '301': 'Moved Permanently', '302': 'Found', '400': 'Bad Request', '404': 'Not Found', '500': 'Internal Server Error' };
export function Agent(options) { this.options = options || {}; }
export const globalAgent = new Agent({ protocol: 'https:' });

const https = { request: request, get: get, ClientRequest: ClientRequest, IncomingMessage: IncomingMessage, Server: Server, createServer: createServer, METHODS: METHODS, STATUS_CODES: STATUS_CODES, Agent: Agent, globalAgent: globalAgent };
export default https;
