
function hexValue(code) {
  if (code >= 48 && code <= 57) return code - 48;
  if (code >= 65 && code <= 70) return code - 55;
  if (code >= 97 && code <= 102) return code - 87;
  return -1;
}

function percentEncode(value) {
  const input = String(value);
  let out = '';
  for (let i = 0; i < input.length; i = i + 1) {
    const ch = input.charAt(i);
    if (ch === ' ') out = out + '%20';
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

export function escape(str) {
  return percentEncode(str);
}

export function unescape(str) {
  return percentDecode(str, true);
}

export function parse(str, sep, eq, options) {
  sep = sep === undefined ? '&' : sep;
  eq = eq === undefined ? '=' : eq;
  const maxKeys = options && options.maxKeys !== undefined ? options.maxKeys : 1000;
  const obj = Object.create(null);
  const input = str === undefined || str === null ? '' : String(str);
  if (input.length === 0) return obj;
  const parts = input.split(sep);
  const count = maxKeys === 0 ? parts.length : Math.min(maxKeys, parts.length);
  for (let i = 0; i < count; i = i + 1) {
    const part = parts[i];
    const idx = part.indexOf(eq);
    const key = unescape(idx >= 0 ? part.substring(0, idx) : part);
    const value = unescape(idx >= 0 ? part.substring(idx + eq.length, part.length) : '');
    if (obj[key] !== undefined) {
      if (Array.isArray(obj[key])) obj[key].push(value);
      else obj[key] = [obj[key], value];
    } else {
      obj[key] = value;
    }
  }
  return obj;
}

export function stringify(obj, sep, eq, options) {
  sep = sep === undefined ? '&' : sep;
  eq = eq === undefined ? '=' : eq;
  const enc = options && options.encodeURIComponent ? options.encodeURIComponent : escape;
  const out = [];
  const keys = Object.keys(obj || {});
  for (let keyIndex = 0; keyIndex < keys.length; keyIndex = keyIndex + 1) {
    const key = keys[keyIndex];
    const value = obj[key];
    if (value === undefined) continue;
    const values = Array.isArray(value) ? value : [value];
    for (let i = 0; i < values.length; i = i + 1) {
      const v = values[i] === null ? '' : (values[i] === 1 ? '1' : (values[i] === 2 ? '2' : String(values[i])));
      out.push(enc(key) + eq + enc(v));
    }
  }
  return out.join(sep);
}

export default { parse, stringify, escape, unescape };
