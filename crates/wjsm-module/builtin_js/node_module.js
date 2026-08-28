// node:module — createRequire / builtinModules / isBuiltin 最小真实面。
// createRequire 与 CJS 模块内建 require 共享同一宿主实现（CjsCreateRequire），
// builtinModules 直接来自宿主注册表，不在 JS 侧维护第二份清单。

function getHost() {
  const host = globalThis.__wjsm_node_module;
  if (!host) throw new Error('wjsm internal module host bridge is not installed');
  return host;
}

const host = getHost();

const moduleNames = host.builtinModules.slice();
moduleNames.sort();
Object.freeze(moduleNames);

export const builtinModules = moduleNames;

export function isBuiltin(specifier) {
  if (typeof specifier !== 'string') return false;
  const canonical = specifier.startsWith('node:') ? specifier.slice(5) : specifier;
  return moduleNames.indexOf(canonical) !== -1;
}

function invalidFilenameError(received) {
  const error = new TypeError(
    "The argument 'filename' must be a file URL object, file URL string, or " +
      'absolute path string. Received ' +
      received,
  );
  error.code = 'ERR_INVALID_ARG_VALUE';
  return error;
}

// 百分号解码（fileURLToPath 的最小子集）：连续 %XX 序列按 UTF-8 字节还原。
function decodePercent(text) {
  if (text.indexOf('%') === -1) return text;
  return text.replace(/(?:%[0-9A-Fa-f]{2})+/g, (run) => {
    const bytes = new Uint8Array(run.length / 3);
    for (let i = 0; i < bytes.length; i += 1) {
      bytes[i] = parseInt(run.slice(i * 3 + 1, i * 3 + 3), 16);
    }
    return new TextDecoder().decode(bytes);
  });
}

// file: URL → 主机路径（Node fileURLToPath 的最小子集：本地文件、百分号解码）。
function pathFromFileUrl(href) {
  const url = new URL(href);
  if (url.protocol !== 'file:') throw invalidFilenameError("'" + href + "'");
  let pathname = decodePercent(url.pathname);
  if (process.platform === 'win32' && /^\/[A-Za-z]:/.test(pathname)) {
    pathname = pathname.slice(1);
  }
  return pathname;
}

function isAbsolutePath(path) {
  if (path.startsWith('/')) return true;
  if (process.platform === 'win32') {
    return /^[A-Za-z]:[\\/]/.test(path) || path.startsWith('\\\\');
  }
  return false;
}

export function createRequire(filename) {
  let path;
  if (typeof filename === 'string') {
    path = filename.startsWith('file:') ? pathFromFileUrl(filename) : filename;
    if (!isAbsolutePath(path)) throw invalidFilenameError("'" + filename + "'");
  } else if (filename && typeof filename === 'object' && typeof filename.href === 'string') {
    path = pathFromFileUrl(filename.href);
  } else {
    throw invalidFilenameError(String(filename));
  }
  return host.createRequire(path);
}

// 非目标：明确抛错，不留 no-op（与 node:vm 的处理一致）。
export function syncBuiltinESMExports() {
  throw new Error('not implemented in wjsm: module.syncBuiltinESMExports');
}

export function findSourceMap() {
  throw new Error('not implemented in wjsm: module.findSourceMap');
}

// 旧式 CJS Module 构造器的最小真实字段面（Node 中已属 legacy API）。
export function Module(id, parent) {
  this.id = id === undefined ? '' : id;
  this.path = '';
  this.exports = {};
  this.filename = null;
  this.loaded = false;
  this.children = [];
  this.parent = parent === undefined ? null : parent;
  this.paths = [];
}

Module.Module = Module;
Module.createRequire = createRequire;
Module.builtinModules = builtinModules;
Module.isBuiltin = isBuiltin;
Module.syncBuiltinESMExports = syncBuiltinESMExports;
Module.findSourceMap = findSourceMap;

export default Module;
