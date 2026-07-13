// node:async_hooks — Node v24.15 公共 API 外形（host-core）。
// 注意：wjsm 对 `this.x =` 在部分 constructor 路径上不稳定，ALS/AR 用工厂对象。

function getHost() {
  const asyncHooksHost = globalThis.__wjsm_node_async_hooks;
  if (!asyncHooksHost) throw new Error('wjsm internal async_hooks host bridge is not installed');
  return asyncHooksHost;
}

const asyncHooksHost = getHost();

export function executionAsyncId() {
  return asyncHooksHost.executionAsyncId();
}

export function triggerAsyncId() {
  return asyncHooksHost.triggerAsyncId();
}

export function executionAsyncResource() {
  return asyncHooksHost.executionAsyncResource();
}

export const asyncWrapProviders = Object.freeze(asyncHooksHost.providers());

export function createHook(options) {
  if (options === null || options === undefined || typeof options !== 'object') {
    const err = new TypeError('The "options" argument must be of type object');
    err.code = 'ERR_INVALID_ARG_TYPE';
    throw err;
  }
  const names = ['init', 'before', 'after', 'destroy', 'promiseResolve'];
  for (let i = 0; i < names.length; i++) {
    const name = names[i];
    if (options[name] !== undefined && typeof options[name] !== 'function') {
      const err = new TypeError('hook.' + name + ' must be a function');
      err.code = 'ERR_ASYNC_CALLBACK';
      throw err;
    }
  }
  if (options.trackPromises !== undefined && typeof options.trackPromises !== 'boolean') {
    const err = new TypeError('The "options.trackPromises" argument must be of type boolean');
    err.code = 'ERR_INVALID_ARG_TYPE';
    throw err;
  }
  if (options.trackPromises === false && typeof options.promiseResolve === 'function') {
    const err = new TypeError(
      "The value of 'options.trackPromises' is invalid. Received false"
    );
    err.code = 'ERR_INVALID_ARG_VALUE';
    throw err;
  }
  return asyncHooksHost.createHook(
    options.init,
    options.before,
    options.after,
    options.destroy,
    options.promiseResolve,
    options.trackPromises !== false
  );
}

export function AsyncResource(type, opts) {
  if (new.target === undefined) {
    throw new TypeError("Class constructor AsyncResource cannot be invoked without 'new'");
  }
  if (typeof type !== 'string') {
    const err = new TypeError('The "type" argument must be of type string');
    err.code = 'ERR_INVALID_ARG_TYPE';
    throw err;
  }
  let triggerAsyncId;
  if (typeof opts === 'number') {
    triggerAsyncId = opts;
  } else if (opts !== undefined && opts !== null) {
    if (typeof opts !== 'object') {
      const err = new TypeError('The "options" argument must be of type object');
      err.code = 'ERR_INVALID_ARG_TYPE';
      throw err;
    }
    triggerAsyncId = opts.triggerAsyncId;
  }
  if (
    triggerAsyncId !== undefined &&
    (!Number.isSafeInteger(triggerAsyncId) || triggerAsyncId < -1)
  ) {
    const err = new RangeError('Invalid triggerAsyncId: ' + triggerAsyncId);
    err.code = 'ERR_INVALID_ASYNC_ID';
    throw err;
  }

  const api = asyncHooksHost.asyncResourceNew(type, opts);
  api.runInAsyncScope = function (fn, thisArg, ...args) {
    if (typeof fn !== 'function') {
      const err = new TypeError('The "fn" argument must be of type function');
      err.code = 'ERR_INVALID_ARG_TYPE';
      throw err;
    }
    asyncHooksHost.asyncResourceEnter(api);
    try {
      return fn.apply(thisArg, args);
    } finally {
      asyncHooksHost.asyncResourceExit(api);
    }
  };
  api.emitDestroy = function () {
    asyncHooksHost.asyncResourceEmitDestroy(api);
    return api;
  };
  api.asyncId = function () {
    return asyncHooksHost.asyncResourceAsyncId(api);
  };
  api.triggerAsyncId = function () {
    return asyncHooksHost.asyncResourceTriggerAsyncId(api);
  };
  api.bind = function (fn, thisArg) {
    if (typeof fn !== 'function') {
      const err = new TypeError('The "fn" argument must be of type function');
      err.code = 'ERR_INVALID_ARG_TYPE';
      throw err;
    }
    const handler = {
      boundThis: thisArg,
      apply(target, receiver, args) {
        const selectedThis = this.boundThis === undefined ? receiver : this.boundThis;
        return api.runInAsyncScope.apply(api, [target, selectedThis].concat(args));
      },
    };
    return new Proxy(fn, handler);
  };
  Object.setPrototypeOf(api, AsyncResource.prototype);
  return api;
}

AsyncResource.bind = function (fn, type, thisArg) {
  if (typeof fn !== 'function') {
    const err = new TypeError('The "fn" argument must be of type function');
    err.code = 'ERR_INVALID_ARG_TYPE';
    throw err;
  }
  const t = type || fn.name || 'bound-anonymous-fn';
  return AsyncResource(t).bind(fn, thisArg);
};

function alsGetStore() {
  return asyncHooksHost.alsGetStore(this.__asyncLocalKey);
}

function alsEnterWith(value) {
  asyncHooksHost.alsEnterWith(this.__asyncLocalKey, value);
}

function alsDisable() {
  asyncHooksHost.alsDisable(this.__asyncLocalKey);
}

function alsRun(value, fn) {
  const hasArgs = arguments.length > 2;
  const args = hasArgs ? Array.prototype.slice.call(arguments, 2) : undefined;
  const prior = asyncHooksHost.alsGetStore(this.__asyncLocalKey);
  if (Object.is(prior, value)) {
    return hasArgs ? fn.apply(undefined, args) : fn();
  }
  asyncHooksHost.alsEnterWith(this.__asyncLocalKey, value);
  try {
    return hasArgs ? fn.apply(undefined, args) : fn();
  } finally {
    asyncHooksHost.alsEnterWith(this.__asyncLocalKey, prior);
  }
}

function alsExit(fn) {
  if (arguments.length === 1) return this.run(undefined, fn);
  const args = Array.prototype.slice.call(arguments, 1);
  return this.run.apply(this, [undefined, fn].concat(args));
}

export function AsyncLocalStorage(options) {
  if (new.target === undefined && !(options === this && arguments.length > 0)) {
    throw new TypeError("Class constructor AsyncLocalStorage cannot be invoked without 'new'");
  }
  const actualOptions = options === this ? arguments[1] : options;
  const hasOptions = actualOptions !== undefined;
  const name = hasOptions && actualOptions.name !== undefined ? String(actualOptions.name) : '';
  const hasDefaultValue = hasOptions && 'defaultValue' in actualOptions;
  const defaultValue = hasDefaultValue ? actualOptions.defaultValue : undefined;
  const key = asyncHooksHost.alsNew(hasDefaultValue, defaultValue, name);
  const api = options === this ? this : {};
  api.__asyncLocalKey = key;
  api.name = name;
  api.getStore = alsGetStore;
  api.enterWith = alsEnterWith;
  api.disable = alsDisable;
  api.run = alsRun;
  api.exit = alsExit;
  Object.setPrototypeOf(api, AsyncLocalStorage.prototype);
  return api;
}

AsyncLocalStorage.bind = function (fn) {
  const frame = asyncHooksHost.alsCaptureFrame();
  return function bound() {
    const args = Array.prototype.slice.call(arguments);
    const prior = asyncHooksHost.alsPushFrame(frame);
    try {
      return fn.apply(undefined, args);
    } finally {
      asyncHooksHost.alsPopFrame(prior);
    }
  };
};

AsyncLocalStorage.snapshot = function () {
  const frame = asyncHooksHost.alsCaptureFrame();
  return function runInSnap(cb) {
    const args = Array.prototype.slice.call(arguments, 1);
    const prior = asyncHooksHost.alsPushFrame(frame);
    try {
      return cb.apply(undefined, args);
    } finally {
      asyncHooksHost.alsPopFrame(prior);
    }
  };
};

export default {
  createHook: createHook,
  executionAsyncId: executionAsyncId,
  triggerAsyncId: triggerAsyncId,
  executionAsyncResource: executionAsyncResource,
  asyncWrapProviders: asyncWrapProviders,
  AsyncResource: AsyncResource,
  AsyncLocalStorage: AsyncLocalStorage,
};
