import { EventEmitter } from 'events';

function schedule(fn) {
  if (typeof queueMicrotask === 'function') queueMicrotask(fn);
  else setTimeout(fn, 0);
}

function callMaybe(fn, self, a, b, c) {
  if (typeof fn !== 'function') return undefined;
  if (c !== undefined) return fn.call(self, a, b, c);
  if (b !== undefined) return fn.call(self, a, b);
  if (a !== undefined) return fn.call(self, a);
  return fn.call(self);
}

function normalizeOptions(options) {
  return options && typeof options === 'object' ? options : {};
}

function emitBufferedData(stream) {
  while (stream._readableFlowing && stream._readableBuffer.length > 0) {
    var chunk = stream._readableBuffer.shift();
    stream.emit('data', chunk);
    var pipes = stream._readablePipes.slice();
    for (var i = 0; i < pipes.length; i = i + 1) {
      if (pipes[i] && pipes[i].write) pipes[i].write(chunk);
    }
  }
  if (stream._readableFlowing && stream.readableEnded && !stream._endEmitted) {
    stream._endEmitted = true;
    stream.emit('end');
    var pipes2 = stream._readablePipes.slice();
    for (var j = 0; j < pipes2.length; j = j + 1) {
      if (pipes2[j] && pipes2[j].end) pipes2[j].end();
    }
  }
}

function finishWritable(stream) {
  if (stream.writableFinished) return;
  stream.writableFinished = true;
  stream.emit('finish');
  stream.emit('close');
}

function setupReadable(obj, options) {
  EventEmitter.call(obj);
  options = normalizeOptions(options);
  obj.readable = true;
  obj.readableEnded = false;
  obj.destroyed = false;
  obj._readableBuffer = [];
  obj._readableFlowing = false;
  obj._endEmitted = false;
  obj._readablePipes = [];
  obj._readableObjectMode = Boolean(options.objectMode || options.readableObjectMode);
  obj._readableHighWaterMark = options.highWaterMark || 16;
}

function setupWritable(obj, options) {
  options = normalizeOptions(options);
  obj.writable = true;
  obj.writableEnded = false;
  obj.writableFinished = false;
  obj._writableBuffer = [];
  obj._writableCorked = 0;
  obj._writableLength = 0;
  obj._writableObjectMode = Boolean(options.objectMode || options.writableObjectMode);
  obj._writableHighWaterMark = options.highWaterMark || 16;
}

export function Readable(options) {
  setupReadable(this, options);
}

export function Writable(options) {
  EventEmitter.call(this);
  setupWritable(this, options);
  this.destroyed = false;
}

export function Duplex(options) {
  EventEmitter.call(this);
  setupReadable(this, options);
  setupWritable(this, options);
}

export function Transform(options) {
  Duplex.call(this, options);
  options = normalizeOptions(options);
  this._transformImpl = typeof options.transform === 'function' ? options.transform : undefined;
  this._flushImpl = typeof options.flush === 'function' ? options.flush : undefined;
}

export function PassThrough(options) {
  Transform.call(this, options);
}

Readable.prototype = Object.create(EventEmitter.prototype);
Readable.prototype.constructor = Readable;
Readable.prototype._read = function () {};
Readable.prototype.push = function (chunk) {
  if (this.destroyed) return false;
  if (chunk === null) {
    this.readableEnded = true;
    var s = this;
    schedule(function () { emitBufferedData(s); });
    return false;
  }
  this._readableBuffer.push(chunk);
  if (this._readableFlowing) { var s2 = this; schedule(function () { emitBufferedData(s2); }); }
  return this._readableBuffer.length < this._readableHighWaterMark;
};
Readable.prototype.read = function () {
  if (this._readableBuffer.length === 0) callMaybe(this._read, this, this._readableHighWaterMark);
  if (this._readableBuffer.length === 0) return this.readableEnded ? null : undefined;
  return this._readableBuffer.shift();
};
Readable.prototype.pipe = function (dest, pipeOpts) {
  if (!dest || typeof dest.write !== 'function') throw new TypeError('dest.write must be a function');
  this._readablePipes.push(dest);
  this.resume();
  var end = !pipeOpts || pipeOpts.end !== false;
  if (end) this.once('end', function () { if (dest.end) dest.end(); });
  if (dest.emit) dest.emit('pipe', this);
  return dest;
};
Readable.prototype.unpipe = function (dest) {
  if (dest === undefined) this._readablePipes = [];
  else {
    var next = [];
    for (var i = 0; i < this._readablePipes.length; i = i + 1) {
      if (this._readablePipes[i] !== dest) next.push(this._readablePipes[i]);
    }
    this._readablePipes = next;
  }
  return this;
};
Readable.prototype.pause = function () { this._readableFlowing = false; return this; };
Readable.prototype.resume = function () { this._readableFlowing = true; var s = this; schedule(function () { emitBufferedData(s); }); return this; };
Readable.prototype.destroy = function (error) {
  if (this.destroyed) return this;
  this.destroyed = true;
  if (error) this.emit('error', error);
  this.emit('close');
  return this;
};
Readable.prototype.on = function (name, listener) {
  var result = EventEmitter.prototype.on.call(this, name, listener);
  if (name === 'data') this._readableFlowing = true;
  return result;
};
Readable.prototype.addListener = Readable.prototype.on;
Readable.prototype.once = function (name, listener) {
  var result = EventEmitter.prototype.once.call(this, name, listener);
  if (name === 'data') this._readableFlowing = true;
  return result;
};

Writable.prototype = Object.create(EventEmitter.prototype);
Writable.prototype.constructor = Writable;
Writable.prototype._write = function (chunk, encoding, callback) { if (callback) callback(); };
Writable.prototype._final = function (callback) { if (callback) callback(); };
Writable.prototype.write = function (chunk, encoding, callback) {
  if (typeof encoding === 'function') { callback = encoding; encoding = undefined; }
  if (this.destroyed) throw new Error('Cannot call write after destroy');
  if (this.writableEnded) throw new Error('write after end');
  var record = { chunk: chunk, encoding: encoding || 'utf8', callback: callback };
  if (this._writableCorked > 0) this._writableBuffer.push(record);
  else this._writeRecord(record);
  this._writableLength = this._writableLength + 1;
  var ok = this._writableLength < this._writableHighWaterMark;
  if (!ok) { var s = this; schedule(function () { s._writableLength = 0; s.emit('drain'); }); }
  return ok;
};
Writable.prototype._writeRecord = function (record) {
  var s = this;
  callMaybe(this._write, this, record.chunk, record.encoding, function (error) {
    if (error) s.emit('error', error);
    if (record.callback) record.callback(error);
  });
};
Writable.prototype.end = function (chunk, encoding, callback) {
  if (typeof chunk === 'function') { callback = chunk; chunk = undefined; encoding = undefined; }
  else if (typeof encoding === 'function') { callback = encoding; encoding = undefined; }
  if (chunk !== undefined) this.write(chunk, encoding);
  this.writableEnded = true;
  var s = this;
  callMaybe(this._final, this, function (error) {
    if (error) s.emit('error', error);
    if (callback) callback(error);
    finishWritable(s);
  });
  return this;
};
Writable.prototype.cork = function () { this._writableCorked = this._writableCorked + 1; };
Writable.prototype.uncork = function () {
  if (this._writableCorked > 0) this._writableCorked = this._writableCorked - 1;
  if (this._writableCorked === 0) {
    while (this._writableBuffer.length > 0) this._writeRecord(this._writableBuffer.shift());
  }
};
Writable.prototype.destroy = Readable.prototype.destroy;

Duplex.prototype = Object.create(Readable.prototype);
Duplex.prototype.constructor = Duplex;
Duplex.prototype._write = Writable.prototype._write;
Duplex.prototype._final = Writable.prototype._final;
Duplex.prototype.write = Writable.prototype.write;
Duplex.prototype._writeRecord = Writable.prototype._writeRecord;
Duplex.prototype.end = Writable.prototype.end;
Duplex.prototype.cork = Writable.prototype.cork;
Duplex.prototype.uncork = Writable.prototype.uncork;

Transform.prototype = Object.create(Duplex.prototype);
Transform.prototype.constructor = Transform;
Transform.prototype._write = function (chunk, encoding, callback) {
  var s = this;
  var transform = this._transformImpl || function (value, enc, done) { this.push(value); if (done) done(); };
  callMaybe(transform, this, chunk, encoding, function (error, data) {
    if (error) s.emit('error', error);
    if (data !== undefined) s.push(data);
    if (callback) callback(error);
  });
};
Transform.prototype.end = function (chunk, encoding, callback) {
  if (typeof chunk === 'function') { callback = chunk; chunk = undefined; encoding = undefined; }
  else if (typeof encoding === 'function') { callback = encoding; encoding = undefined; }
  if (chunk !== undefined) this.write(chunk, encoding);
  this.writableEnded = true;
  var s = this;
  var flush = this._flushImpl || function (done) { if (done) done(); };
  callMaybe(flush, this, function (error, data) {
    if (error) s.emit('error', error);
    if (data !== undefined) s.push(data);
    s.push(null);
    if (callback) callback(error);
    finishWritable(s);
  });
  return this;
};

PassThrough.prototype = Object.create(Transform.prototype);
PassThrough.prototype.constructor = PassThrough;

export function finished(stream, callback) {
  return new Promise(function (resolve, reject) {
    function done(error) {
      cleanup();
      if (callback) callback(error);
      if (error) reject(error); else resolve();
    }
    function cleanup() {
      if (stream.off) { stream.off('error', done); stream.off('end', onEnd); stream.off('finish', onFinish); stream.off('close', onClose); }
    }
    function onEnd() { if (!stream.writable || stream.writableFinished || stream.writableEnded) done(); }
    function onFinish() { if (!stream.readable || stream.readableEnded) done(); }
    function onClose() { if (stream.destroyed) done(); }
    if (stream.on) {
      stream.once('error', done);
      stream.once('end', onEnd);
      stream.once('finish', onFinish);
      stream.once('close', onClose);
    }
    if ((stream.readableEnded || !stream.readable) && (stream.writableFinished || stream.writableEnded || !stream.writable)) schedule(function () { done(); });
  });
}

export function pipeline() {
  var args = [];
  for (var i = 0; i < arguments.length; i = i + 1) args.push(arguments[i]);
  var callback = undefined;
  if (typeof args[args.length - 1] === 'function') callback = args.pop();
  var streams = args;
  for (var i = 0; i + 1 < streams.length; i = i + 1) streams[i].pipe(streams[i + 1]);
  var last = streams[streams.length - 1];
  return finished(last, callback);
}

export const Stream = EventEmitter;

var streamDefault = {
  Readable: Readable,
  Writable: Writable,
  Duplex: Duplex,
  Transform: Transform,
  PassThrough: PassThrough,
  Stream: Stream,
  pipeline: pipeline,
  finished: finished
};
export default streamDefault;
