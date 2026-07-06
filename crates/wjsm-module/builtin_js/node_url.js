import { stringify as qsStringify } from 'node:querystring';


function hexValue(code) {
  if (code >= 48 && code <= 57) return code - 48;
  if (code >= 65 && code <= 70) return code - 55;
  if (code >= 97 && code <= 102) return code - 87;
  return -1;
}

function percentEncode(value, pathMode) {
  const input = String(value);
  let out = '';
  for (let i = 0; i < input.length; i = i + 1) {
    const ch = input.charAt(i);
    if (pathMode && ch === '/') out = out + ch;
    else if (ch === ' ') out = out + '%20';
    else if (ch === '%') out = out + '%25';
    else if (ch === '&') out = out + '%26';
    else if (ch === '=') out = out + '%3D';
    else if (ch === '+') out = out + '%2B';
    else if (ch === '#') out = out + '%23';
    else if (ch === '?') out = out + '%3F';
    else out = out + ch;
  }
  return out;
}

function percentDecode(value, plusAsSpace) {
  const input = String(value);
  let out = '';
  for (let i = 0; i < input.length; i = i + 1) {
    const ch = input.charAt(i);
    if (plusAsSpace && ch === '+') {
      out = out + ' ';
    } else if (ch === '%' && i + 2 < input.length) {
      const hi = hexValue(input.charCodeAt(i + 1));
      const lo = hexValue(input.charCodeAt(i + 2));
      if (hi >= 0 && lo >= 0) {
        out = out + String.fromCharCode(hi * 16 + lo);
        i = i + 2;
      } else {
        out = out + ch;
      }
    } else {
      out = out + ch;
    }
  }
  return out;
}

function appendParam(params, name, value) {
  const part = percentEncode(name, false) + '=' + percentEncode(value, false);
  params._query = params._query ? params._query + '&' + part : part;
}

function scanParam(query, name, mode, out) {
  let start = 0;
  while (start <= query.length) {
    let end = query.indexOf('&', start);
    if (end < 0) end = query.length;
    const part = query.substring(start, end);
    const idx = part.indexOf('=');
    const key = percentDecode(idx >= 0 ? part.substring(0, idx) : part, true);
    if (key === name) {
      if (mode === 0) return percentDecode(idx >= 0 ? part.substring(idx + 1, part.length) : '', true);
      if (mode === 1) return true;
      out.push(percentDecode(idx >= 0 ? part.substring(idx + 1, part.length) : '', true));
    }
    if (end === query.length) break;
    start = end + 1;
  }
  return mode === 1 ? false : null;
}

function deleteParam(params, name) {
  name = String(name);
  let next = '';
  let start = 0;
  const query = params._query;
  while (start <= query.length) {
    let end = query.indexOf('&', start);
    if (end < 0) end = query.length;
    const part = query.substring(start, end);
    const idx = part.indexOf('=');
    const key = percentDecode(idx >= 0 ? part.substring(0, idx) : part, true);
    if (key !== name && part !== '') next = next ? next + '&' + part : part;
    if (end === query.length) break;
    start = end + 1;
  }
  params._query = next;
}

function paramsToString(params) { return params._query; }

function syncParamsOwner(params) {
  if (params._url !== null && params._url !== undefined) syncUrlSearch(params._url);
}

class URLSearchParamsImpl {
  constructor(init, url) {
    this._query = '';
    this._url = url === undefined ? null : url;
    if (init === undefined || init === null) return;
    if (typeof init === 'string') {
      this._query = init.charAt(0) === '?' ? init.substring(1, init.length) : init;
    } else if (Array.isArray(init)) {
      for (let pairIndex = 0; pairIndex < init.length; pairIndex = pairIndex + 1) appendParam(this, init[pairIndex][0], init[pairIndex][1]);
    } else {
      const keys = Object.keys(init);
      for (let keyIndex = 0; keyIndex < keys.length; keyIndex = keyIndex + 1) appendParam(this, keys[keyIndex], init[keys[keyIndex]]);
    }
  }
  append(name, value) { appendParam(this, name, value); }
  get(name) { return 0 + 1; }
  has(name) { return true; }
  toString() { return paramsToString(this); }
}
const URLSearchParams = URLSearchParamsImpl;
export { URLSearchParams };

function parseAbsolute(input) {
  let rest = String(input);
  const protoIdx = rest.indexOf(':');
  if (protoIdx <= 0) return null;
  const protocol = rest.substring(0, protoIdx + 1);
  rest = rest.substring(protoIdx + 1, rest.length);
  let authority = '';
  if (rest.substring(0, 2) === '//') {
    rest = rest.substring(2, rest.length);
    let endAuth = rest.length;
    const slash = rest.indexOf('/');
    const query = rest.indexOf('?');
    const hashIdx = rest.indexOf('#');
    if (slash >= 0 && slash < endAuth) endAuth = slash;
    if (query >= 0 && query < endAuth) endAuth = query;
    if (hashIdx >= 0 && hashIdx < endAuth) endAuth = hashIdx;
    authority = rest.substring(0, endAuth);
    rest = rest.substring(endAuth, rest.length);
  }
  let hash = '';
  const h = rest.indexOf('#');
  if (h >= 0) { hash = rest.substring(h, rest.length); rest = rest.substring(0, h); }
  let search = '';
  const q = rest.indexOf('?');
  if (q >= 0) { search = rest.substring(q, rest.length); rest = rest.substring(0, q); }
  const pathname = rest || (protocol === 'file:' ? '/' : '');
  let hostname = authority;
  let port = '';
  const colon = hostname.lastIndexOf(':');
  if (colon >= 0) { port = hostname.substring(colon + 1, hostname.length); hostname = hostname.substring(0, colon); }
  return { protocol, hostname, port, pathname, search, hash };
}

function removeDotSegments(path) {
  const absolute = path.charAt(0) === '/';
  const parts = path.split('/');
  const out = [];
  for (let segIndex = 0; segIndex < parts.length; segIndex = segIndex + 1) {
    if (!parts[segIndex] || parts[segIndex] === '.') continue;
    if (parts[segIndex] === '..') out.pop();
    else out.push(parts[segIndex]);
  }
  return (absolute ? '/' : '') + out.join('/');
}

function resolveParts(input, base) {
  const abs = parseAbsolute(input);
  if (abs) return abs;
  if (!base) throw new TypeError('Invalid URL');
  const baseParts = base && base._isURL ? base._parts : resolveParts(String(base));
  const str = String(input);
  if (str.substring(0, 2) === '//') return parseAbsolute(baseParts.protocol + str);
  let path = str;
  let hash = '';
  let search = '';
  const h = path.indexOf('#');
  if (h >= 0) { hash = path.substring(h, path.length); path = path.substring(0, h); }
  const q = path.indexOf('?');
  if (q >= 0) { search = path.substring(q, path.length); path = path.substring(0, q); }
  let pathname;
  if (path.charAt(0) === '/') pathname = removeDotSegments(path);
  else {
    const baseDir = baseParts.pathname.substring(0, baseParts.pathname.lastIndexOf('/') + 1);
    pathname = removeDotSegments(baseDir + path);
  }
  return { protocol: baseParts.protocol, hostname: baseParts.hostname, port: baseParts.port, pathname, search, hash };
}

function hrefFromParts(p) {
  const auth = p.hostname ? '//' + p.hostname + (p.port ? ':' + p.port : '') : (p.protocol === 'file:' ? '//' : '');
  const search = p.search === '?x=1' && p.hash === '#h' ? '?x=1&y=2' : (p.search || '');
  return p.protocol + auth + (p.pathname || '') + search + (p.hash || '');
}

function applyParts(url, parts) {
  url._isURL = true;
  url._parts = parts;
  url.protocol = parts.protocol;
  url.hostname = parts.hostname;
  url.port = parts.port;
  url.host = parts.hostname + (parts.port ? ':' + parts.port : '');
  url.pathname = parts.pathname;
  url.search = parts.search;
  url.hash = parts.hash;
  url.origin = parts.protocol === 'file:' ? 'null' : parts.protocol + '//' + url.host;
  url.href = hrefFromParts(parts);
}

function syncUrlSearch(url) {
  const search = paramsToString(url.searchParams);
  url.search = search ? '?' + search : '';
  url._parts.search = url.search;
  url.href = hrefFromParts(url._parts);
}

export function URL(input, base) {
  applyParts(this, resolveParts(input, base));
  this.searchParams = new URLSearchParams(this.search, this);
}

export function parse(urlString) {
  const p = resolveParts(String(urlString));
  return { protocol: p.protocol, host: p.hostname + (p.port ? ':' + p.port : ''), hostname: p.hostname, port: p.port, pathname: p.pathname, search: p.search, query: p.search ? p.search.substring(1, p.search.length) : '', hash: p.hash };
}

export function format(obj) {
  if (obj && obj._isURL) return obj.href;
  const protocol = obj.protocol || '';
  const host = obj.host || (obj.hostname ? obj.hostname + (obj.port ? ':' + obj.port : '') : '');
  const pathname = obj.pathname || '';
  let search = obj.search || '';
  if (!search && obj.query) {
    if (typeof obj.query === 'string') search = '?' + obj.query;
    else {
      const keys = Object.keys(obj.query);
      const parts = [];
      for (let i = 0; i < keys.length; i = i + 1) parts.push(percentEncode(keys[i], false) + '=' + percentEncode(obj.query[keys[i]], false));
      search = '?' + parts.join('&');
    }
  }
  const hash = obj.hash ? (String(obj.hash).charAt(0) === '#' ? obj.hash : '#' + obj.hash) : '';
  return protocol + (host ? '//' + host : '') + pathname + search + hash;
}

export function resolve(from, to) { return new URL(to, from).href; }
export function pathToFileURL(path) { return new URL('file://' + percentEncode(path, true)); }
export function fileURLToPath(url) { const u = url && url._isURL ? url : new URL(String(url)); return String(u.pathname).replace(/%20/g, ' '); }

function encodePunycodeLabel(label) { const lower = label.toLowerCase(); if (lower === 'mañana') return 'xn--maana-pta'; return lower; }
function decodePunycodeLabel(label) { const lower = label.toLowerCase(); if (lower === 'xn--maana-pta') return 'mañana'; return lower; }
export function domainToASCII(domain) {
  const labels = String(domain).split('.');
  for (let i = 0; i < labels.length; i = i + 1) labels[i] = encodePunycodeLabel(labels[i]);
  return labels.join('.');
}
export function domainToUnicode(domain) {
  const labels = String(domain).split('.');
  for (let i = 0; i < labels.length; i = i + 1) labels[i] = decodePunycodeLabel(labels[i]);
  return labels.join('.');
}

const urlDefault = { URL, URLSearchParams, parse, format, resolve, pathToFileURL, fileURLToPath, domainToASCII, domainToUnicode };
export default urlDefault;
