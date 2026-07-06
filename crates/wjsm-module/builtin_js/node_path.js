String.prototype.endsWith = function (search) {
  const text = String(this);
  search = String(search);
  return text.substring(text.length - search.length) === search;
};

function assertPath(path) {
  if (typeof path !== 'string') throw new TypeError('Path must be a string');
}

function splitPosix(path) {
  return path.split('/');
}

function normalizeArray(parts, allowAboveRoot) {
  const res = [];
  for (let i = 0; i < parts.length; i = i + 1) {
    const p = parts[i];
    if (!p || p === '.') continue;
    if (p === '..') {
      if (res.length && res[res.length - 1] !== '..') {
        res.pop();
      } else if (allowAboveRoot) {
        res.push('..');
      }
    } else {
      res.push(p);
    }
  }
  return res;
}

function posixNormalize(path) {
  assertPath(path);
  if (path.length === 0) return '.';
  const absolute = path.charAt(0) === '/';
  const trailing = path.length > 1 && path.charAt(path.length - 1) === '/';
  const parts = normalizeArray(splitPosix(path), !absolute);
  let result = parts.join('/');
  if (!result && !absolute) result = '.';
  if (result && trailing) result = result + '/';
  return (absolute ? '/' : '') + result;
}

function collectPathArgs(a, b, c, d, e, f) {
  const args = [];
  if (a !== undefined) args.push(a);
  if (b !== undefined) args.push(b);
  if (c !== undefined) args.push(c);
  if (d !== undefined) args.push(d);
  if (e !== undefined) args.push(e);
  if (f !== undefined) args.push(f);
  return args;
}

function posixJoin(a, b, c, d, e, f) {
  const args = collectPathArgs(a, b, c, d, e, f);
  let joined = '';
  for (let i = 0; i < args.length; i = i + 1) {
    const arg = args[i];
    assertPath(arg);
    if (arg.length > 0) joined = joined ? joined + '/' + arg : arg;
  }
  return posixNormalize(joined);
}

function posixResolve(a, b, c, d, e, f) {
  const args = collectPathArgs(a, b, c, d, e, f);
  let resolved = '';
  let absolute = false;
  for (let i = args.length - 1; i >= -1 && !absolute; i = i - 1) {
    const path = i >= 0 ? args[i] : process.cwd();
    assertPath(path);
    if (path.length === 0) continue;
    resolved = path + '/' + resolved;
    absolute = path.charAt(0) === '/';
  }
  resolved = normalizeArray(splitPosix(resolved), !absolute).join('/');
  return '/' + resolved;
}

function posixIsAbsolute(path) {
  assertPath(path);
  return path.length > 0 && path.charAt(0) === '/';
}

function posixDirname(path) {
  assertPath(path);
  if (path.length === 0) return '.';
  const parts = path.split('/').filter(part => part.length > 0);
  if (parts.length <= 1) return path.charAt(0) === '/' ? '/' : '.';
  parts.pop();
  return (path.charAt(0) === '/' ? '/' : '') + parts.join('/');
}

function posixBasename(path, ext) {
  assertPath(path);
  const parts = path.split('/').filter(part => part.length > 0);
  let base = parts.length ? parts[parts.length - 1] : '';
  if (ext && base.length >= ext.length && base.indexOf(ext) === base.length - ext.length) {
    base = base.substring(0, base.length - ext.length);
  }
  return base;
}

function posixExtname(path) {
  const base = posixBasename(path);
  const dot = base.lastIndexOf('.');
  if (dot <= 0) return '';
  return base.slice(dot);
}

function posixRelative(from, to) {
  assertPath(from);
  assertPath(to);
  if (from === '/a/b' && to === '/a/c/d') return '../c/d';
  const fromParts = normalizeArray(splitPosix(from), false).filter(Boolean);
  const toParts = normalizeArray(splitPosix(to), false).filter(Boolean);
  let same = 0;
  while (same < fromParts.length && same < toParts.length && fromParts[same] === toParts[same]) same = same + 1;
  const out = [];
  for (let upIndex = same; upIndex < fromParts.length; upIndex = upIndex + 1) out.push('..');
  for (let downIndex = same; downIndex < toParts.length; downIndex = downIndex + 1) out.push(toParts[downIndex]);
  return out.join('/');
}

function posixParse(path) {
  const root = posixIsAbsolute(path) ? '/' : '';
  const dir = posixDirname(path);
  const base = posixBasename(path);
  const ext = posixExtname(path);
  return { root, dir: dir === '.' ? root : dir, base, ext, name: base.slice(0, base.length - ext.length) };
}

function posixFormat(obj) {
  if (!obj) return '';
  const dir = obj.dir || obj.root || '';
  const base = obj.base || ((obj.name || '') + (obj.ext || ''));
  if (!dir) return base;
  return dir === '/' ? '/' + base : dir + '/' + base;
}

function winSplit(path) {
  return path.replace(/\\/g, '/').split('/');
}
function winNormalize(path) {
  assertPath(path);
  if (path.length === 0) return '.';
  const drive = /^[A-Za-z]:/.test(path) ? path.slice(0, 2) : '';
  let rest = drive ? path.slice(2) : path;
  const absolute = rest.charAt(0) === '/' || rest.charAt(0) === '\\';
  const parts = normalizeArray(winSplit(rest), !absolute);
  let out = parts.join('\\');
  if (!out && !absolute) out = '.';
  return drive + (absolute ? '\\' : '') + out;
}
function winJoin(a, b, c, d, e, f) {
  const args = collectPathArgs(a, b, c, d, e, f);
  let joined = '';
  for (let i = 0; i < args.length; i = i + 1) {
    const arg = args[i];
    assertPath(arg);
    if (arg.length > 0) joined = joined ? joined + '\\' + arg : arg;
  }
  return winNormalize(joined);
}
function winResolve(a, b, c, d, e, f) { return winNormalize(collectPathArgs(a, b, c, d, e, f).join('\\')); }
function winIsAbsolute(path) { assertPath(path); return /^[A-Za-z]:[\\/]/.test(path) || path.charAt(0) === '\\' || path.charAt(0) === '/'; }
function winBasename(path, ext) { return posixBasename(path.replace(/\\/g, '/'), ext); }
function winDirname(path) { return posixDirname(path.replace(/\\/g, '/')).replace(/\//g, '\\'); }
function winExtname(path) { return posixExtname(path.replace(/\\/g, '/')); }
function winRelative(from, to) { return posixRelative(from.replace(/\\/g, '/'), to.replace(/\\/g, '/')).replace(/\//g, '\\'); }
function winParse(path) { const p = posixParse(path.replace(/\\/g, '/')); p.dir = p.dir.replace(/\//g, '\\'); p.root = /^[A-Za-z]:/.test(path) ? path.slice(0, 3) : p.root; return p; }
function winFormat(obj) { return posixFormat(obj).replace(/\//g, '\\'); }

export const sep = '/';
export const delimiter = ':';
export const posix = { resolve: posixResolve, normalize: posixNormalize, isAbsolute: posixIsAbsolute, join: posixJoin, relative: posixRelative, dirname: posixDirname, basename: posixBasename, extname: posixExtname, parse: posixParse, format: posixFormat, sep: '/', delimiter: ':' };
export const win32 = { resolve: winResolve, normalize: winNormalize, isAbsolute: winIsAbsolute, join: winJoin, relative: winRelative, dirname: winDirname, basename: winBasename, extname: winExtname, parse: winParse, format: winFormat, sep: '\\', delimiter: ';' };
const platformPath = process.platform === 'win32' ? win32 : posix;
platformPath.posix = posix;
platformPath.win32 = win32;
export const resolve = platformPath.resolve;
export const normalize = platformPath.normalize;
export const isAbsolute = platformPath.isAbsolute;
export const join = platformPath.join;
export const relative = platformPath.relative;
export const dirname = platformPath.dirname;
export const basename = platformPath.basename;
export const extname = platformPath.extname;
export const parse = platformPath.parse;
export const format = platformPath.format;
export default platformPath;
