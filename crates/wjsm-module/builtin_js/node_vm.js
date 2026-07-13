// node:vm API 外形：Script / runIn* / compileFunction / constants
// 实际 realm 语义由 globalThis.__wjsm_node_vm host bridge 实现。

function getHost() {
  const vmHost = globalThis.__wjsm_node_vm;
  if (!vmHost) throw new Error('wjsm internal vm host bridge is not installed');
  return vmHost;
}

const vmHost = getHost();

export const constants = {
  USE_MAIN_CONTEXT_DEFAULT_LOADER: 0,
  DONT_CONTEXTIFY: 1,
};

export function createContext(sandbox, options) {
  return vmHost.createContext(sandbox, options);
}

export function isContext(sandbox) {
  return vmHost.isContext(sandbox);
}

export function runInContext(code, contextifiedSandbox, options) {
  return vmHost.runInContext(code, contextifiedSandbox, options);
}

export function runInNewContext(code, sandbox, options) {
  return vmHost.runInNewContext(code, sandbox, options);
}

export function runInThisContext(code, options) {
  return vmHost.runInThisContext(code, options);
}

export function compileFunction(code, params, options) {
  return vmHost.compileFunction(code, params, options);
}

export function Script(code, options) {
  if (!(this instanceof Script)) {
    return new Script(code, options);
  }
  this.__code = code;
  this.__options = options;
}

Script.prototype.runInContext = function (contextifiedSandbox, options) {
  return vmHost.scriptRunInContext(this.__code, contextifiedSandbox, options);
};

Script.prototype.runInNewContext = function (sandbox, options) {
  return vmHost.scriptRunInNewContext(this.__code, sandbox, options);
};

Script.prototype.runInThisContext = function (options) {
  return vmHost.scriptRunInThisContext(this.__code, options);
};

// 非目标：明确抛错，不留 no-op
export function measureMemory() {
  throw new Error('not implemented in wjsm: vm.measureMemory');
}

export function SourceTextModule() {
  throw new Error('not implemented in wjsm: vm.SourceTextModule');
}

export function SyntheticModule() {
  throw new Error('not implemented in wjsm: vm.SyntheticModule');
}

export default {
  Script,
  createContext,
  isContext,
  runInContext,
  runInNewContext,
  runInThisContext,
  compileFunction,
  constants,
  measureMemory,
  SourceTextModule,
  SyntheticModule,
};
