// node:vm API 外形：Script / runIn* / compileFunction / constants
// 实际 realm 语义由 globalThis.__wjsm_node_vm host bridge 实现。

function getHost() {
  const host = globalThis.__wjsm_node_vm;
  if (!host) throw new Error('wjsm internal vm host bridge is not installed');
  return host;
}

const host = getHost();

export const constants = {
  USE_MAIN_CONTEXT_DEFAULT_LOADER: 0,
  DONT_CONTEXTIFY: 1,
};

export function createContext(sandbox, options) {
  return host.createContext(sandbox, options);
}

export function isContext(sandbox) {
  return host.isContext(sandbox);
}

export function runInContext(code, contextifiedSandbox, options) {
  return host.runInContext(code, contextifiedSandbox, options);
}

export function runInNewContext(code, sandbox, options) {
  return host.runInNewContext(code, sandbox, options);
}

export function runInThisContext(code, options) {
  return host.runInThisContext(code, options);
}

export function compileFunction(code, params, options) {
  return host.compileFunction(code, params, options);
}

export function Script(code, options) {
  if (!(this instanceof Script)) {
    return new Script(code, options);
  }
  this.__code = code;
  this.__options = options;
}

Script.prototype.runInContext = function (contextifiedSandbox, options) {
  return host.scriptRunInContext(this.__code, contextifiedSandbox, options);
};

Script.prototype.runInNewContext = function (sandbox, options) {
  return host.scriptRunInNewContext(this.__code, sandbox, options);
};

Script.prototype.runInThisContext = function (options) {
  return host.scriptRunInThisContext(this.__code, options);
};

export default {
  Script,
  createContext,
  isContext,
  runInContext,
  runInNewContext,
  runInThisContext,
  compileFunction,
  constants,
};
