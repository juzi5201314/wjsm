function collectArgs(a, b, c, d, e) {
  const args = [];
  if (a !== undefined) args.push(a);
  if (b !== undefined) args.push(b);
  if (c !== undefined) args.push(c);
  if (d !== undefined) args.push(d);
  if (e !== undefined) args.push(e);
  return args;
}

function callListener(listener, receiver, args) {
  if (args.length === 0) return listener.call(receiver);
  if (args.length === 1) return listener.call(receiver, args[0]);
  if (args.length === 2) return listener.call(receiver, args[0], args[1]);
  if (args.length === 3) return listener.call(receiver, args[0], args[1], args[2]);
  if (args.length === 4) return listener.call(receiver, args[0], args[1], args[2], args[3]);
  return listener.call(receiver, args[0], args[1], args[2], args[3], args[4]);
}

function list(emitter, name, create) {
  let arr = emitter._events[name];
  if (!arr && create) { arr = []; emitter._events[name] = arr; }
  return arr;
}

function checkListener(listener) {
  if (typeof listener !== 'function') throw new TypeError('listener must be a function');
}

function eventOn(name, listener) {
  list(this, name, true).push(listener);
  return this;
}

function eventPrependListener(name, listener) {
  list(this, name, true).unshift(listener);
  return this;
}

function eventOnce(name, listener) {
  list(this, name, true).push(listener);
  list({ _events: this._onceEvents }, name, true).push(listener);
  return this;
}

function eventPrependOnceListener(name, listener) {
  list(this, name, true).unshift(listener);
  list({ _events: this._onceEvents }, name, true).unshift(listener);
  return this;
}

function removeFromList(arr, listener) {
  if (!arr) return;
  for (let removeIndex = arr.length - 1; removeIndex >= 0; removeIndex = removeIndex - 1) {
    if (arr[removeIndex] === listener) { arr.splice(removeIndex, 1); break; }
  }
}

function eventRemoveListener(name, listener) {
  const arr = list(this, name, false);
  if (!arr) return this;
  removeFromList(arr, listener);
  removeFromList(this._onceEvents[name], listener);
  if (arr.length === 0) delete this._events[name];
  if (this._onceEvents[name] && this._onceEvents[name].length === 0) delete this._onceEvents[name];
  return this;
}

function eventRemoveAllListeners(name) {
  if (name === undefined) { this._events = {}; this._onceEvents = {}; }
  else { delete this._events[name]; delete this._onceEvents[name]; }
  return this;
}

function eventEmit(name, a, b, c, d, e) {
  const args = collectArgs(a, b, c, d, e);
  const arr = list(this, name, false);
  if ((!arr || arr.length === 0) && name === 'error') {
    const err = args.length > 0 ? args[0] : undefined;
    throw err instanceof Error ? err : new Error(String(err));
  }
  if (!arr || arr.length === 0) return false;
  for (let emitIndex = 0; emitIndex < arr.length; emitIndex = emitIndex + 1) callListener(arr[emitIndex], this, args);
  const onceArr = this._onceEvents[name];
  if (onceArr) {
    for (let onceIndex = 0; onceIndex < onceArr.length; onceIndex = onceIndex + 1) removeFromList(arr, onceArr[onceIndex]);
    delete this._onceEvents[name];
    if (arr.length === 0) delete this._events[name];
  }
  return true;
}

function eventListeners(name) {
  const arr = list(this, name, false) || [];
  const out = [];
  for (let i = 0; i < arr.length; i = i + 1) out.push(arr[i]);
  return out;
}

function eventRawListeners(name) {
  const arr = list(this, name, false) || [];
  const out = [];
  for (let i = 0; i < arr.length; i = i + 1) out.push(arr[i]);
  return out;
}

function eventListenerCount(name) {
  const arr = list(this, name, false);
  return arr ? arr.length : 0;
}

function eventSetMaxListeners(n) {
  if (typeof n !== 'number' || !isFinite(n) || n < 0) throw new RangeError('n must be a non-negative number');
  this._maxListeners = n;
  return this;
}

function eventGetMaxListeners() { return this._maxListeners; }
function eventEventNames() { return Object.keys(this._events); }

export function EventEmitter() {
  const self = this;
  self._events = {};
  self._onceEvents = {};
  self._maxListeners = EventEmitter.defaultMaxListeners;
  self.captureRejections = false;
  self.on = function (name, listener) { list(self, name, true).push(listener); return self; };
  self.addListener = self.on;
  self.prependListener = function (name, listener) { list(self, name, true).unshift(listener); return self; };
  self.once = function (name, listener) {
    list(self, name, true).push(listener);
    list({ _events: self._onceEvents }, name, true).push(listener);
    return self;
  };
  self.prependOnceListener = function (name, listener) {
    list(self, name, true).unshift(listener);
    list({ _events: self._onceEvents }, name, true).unshift(listener);
    return self;
  };
  self.off = function (name, listener) { return eventRemoveListener.call(self, name, listener); };
  self.removeListener = self.off;
  self.removeAllListeners = function (name) { return eventRemoveAllListeners.call(self, name); };
  self.emit = function (name, a, b, c, d, e) { return eventEmit.call(self, name, a, b, c, d, e); };
  self.listeners = function (name) { return eventListeners.call(self, name); };
  self.rawListeners = function (name) { return eventRawListeners.call(self, name); };
  self.listenerCount = function (name) { return eventListenerCount.call(self, name); };
  self.eventNames = function () { return eventEventNames.call(self); };
  self.setMaxListeners = function (n) { return eventSetMaxListeners.call(self, n); };
  self.getMaxListeners = function () { return eventGetMaxListeners.call(self); };
}
EventEmitter.defaultMaxListeners = 10;
EventEmitter.captureRejections = false;

export function once(emitter, name) {
  return new Promise((resolve, reject) => {
    function done(a, b, c, d, e) {
      cleanup();
      resolve(collectArgs(a, b, c, d, e));
    }
    function onError(err) {
      cleanup();
      reject(err);
    }
    function cleanup() {
      emitter.off(name, done);
      if (name !== 'error') emitter.off('error', onError);
    }
    emitter.once(name, done);
    if (name !== 'error') emitter.once('error', onError);
  });
}

const EventEmitterDefault = EventEmitter;
export default EventEmitterDefault;
