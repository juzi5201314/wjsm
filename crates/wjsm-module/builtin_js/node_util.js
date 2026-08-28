import typesObject from 'node:util/types';

export function inherits(constructor, superConstructor) {
  if (typeof constructor !== 'function' || typeof superConstructor !== 'function') {
    throw new TypeError('The "constructor" and "superConstructor" arguments must be functions');
  }
  constructor.super_ = superConstructor;
  const proto = Object.create(superConstructor.prototype);
  proto.constructor = constructor;
  constructor.prototype = proto;
}

export function inspect(obj, opts) {
  const depth = opts && opts.depth !== undefined ? opts.depth : 2;
  const seen = [];
  function inner(value, level) {
    if (value === null) return 'null';
    if (typeof value === 'string') return "'" + value + "'";
    if (typeof value === 'number') return numberToString(value);
    if (typeof value === 'bigint') return String(value) + 'n';
    if (typeof value !== 'object') return String(value);
    if (seen.indexOf(value) >= 0) return '[Circular]';
    if (level < 0) return Array.isArray(value) ? '[Array]' : '[Object]';
    seen.push(value);
    let result;
    if (Array.isArray(value)) {
      result = '[ ' + value.map(v => inner(v, level - 1)).join(', ') + ' ]';
    } else if (value instanceof Map) {
      const parts = [];
      value.forEach((v, k) => parts.push(inner(k, level - 1) + ' => ' + inner(v, level - 1)));
      result = 'Map(' + value.size + ') { ' + parts.join(', ') + ' }';
    } else if (value instanceof Set) {
      const parts = [];
      value.forEach(v => parts.push(inner(v, level - 1)));
      result = 'Set(' + value.size + ') { ' + parts.join(', ') + ' }';
    } else if (value instanceof Date) {
      result = value.toString();
    } else if (value instanceof RegExp) {
      result = value.toString();
    } else {
      const keys = Object.keys(value);
      result = '{ ' + keys.map(k => k + ': ' + inner(value[k], level - 1)).join(', ') + ' }';
    }
    seen.pop();
    return result;
  }
  return inner(obj, depth);
}

// Node：-0 需渲染为 '-0'，String(-0) 会丢符号。
function numberToString(value) {
  if (value === 0 && 1 / value === -Infinity) return '-0';
  return String(value);
}

// Node util.format 的 %s：数字/bigint 走数字渲染；原始值走 String；
// 无自定义 toString 的对象走浅层 inspect（嵌套显示 [Object]/[Array]）。
function formatStringArg(arg) {
  if (typeof arg === 'number') return numberToString(arg);
  if (typeof arg === 'bigint') return String(arg) + 'n';
  if (typeof arg !== 'object' || arg === null) return String(arg);
  const toStr = arg.toString;
  if (typeof toStr === 'function' && toStr !== Object.prototype.toString && toStr !== Array.prototype.toString) {
    return String(arg);
  }
  return inspect(arg, { depth: 0 });
}

// %d/%i：symbol 不可转数字，Node 渲染为 'NaN'；bigint 保留 'n' 后缀。
function formatNumericArg(arg, parse) {
  if (typeof arg === 'bigint') return String(arg) + 'n';
  if (typeof arg === 'symbol') return 'NaN';
  return numberToString(parse(arg));
}

function isFormatCode(code) {
  return code === 's' || code === 'd' || code === 'i' || code === 'f'
    || code === 'j' || code === 'o' || code === 'O' || code === 'c';
}

// Node formatWithOptionsInternal：保留全部实参（含 undefined），字符串首参做
// 占位符替换，剩余实参以空格追加——字符串原样、其余走 inspect。
export function format(...args) {
  const first = args[0];
  let index = 1;
  let out = '';
  let join = '';
  if (typeof first === 'string') {
    // Node：仅有格式串时原样返回，不做任何占位符处理。
    if (args.length === 1) return first;
    for (let i = 0; i < first.length; i = i + 1) {
      const ch = first.charAt(i);
      if (ch !== '%' || i + 1 >= first.length) {
        out = out + ch;
        continue;
      }
      const code = first.charAt(i + 1);
      i = i + 1;
      if (code === '%') { out = out + '%'; continue; }
      // 未知指令不消耗实参；实参耗尽时占位符保留字面。
      if (!isFormatCode(code) || index >= args.length) { out = out + '%' + code; continue; }
      const arg = args[index];
      index = index + 1;
      if (code === 's') out = out + formatStringArg(arg);
      else if (code === 'd') out = out + formatNumericArg(arg, Number);
      else if (code === 'i') out = out + formatNumericArg(arg, parseInt);
      else if (code === 'f') out = out + (typeof arg === 'symbol' ? 'NaN' : numberToString(parseFloat(arg)));
      else if (code === 'j') {
        try { out = out + JSON.stringify(arg); } catch (err) { out = out + '[Circular]'; }
      } else if (code === 'o' || code === 'O') out = out + inspect(arg);
      // %c：CSS 指令在非浏览器环境消耗实参但不输出。
    }
    join = ' ';
  } else {
    index = 0;
  }
  while (index < args.length) {
    const value = args[index];
    out = out + join;
    out = out + (typeof value === 'string' ? value : inspect(value));
    join = ' ';
    index = index + 1;
  }
  return out;
}

export function deprecate(fn, msg) {
  let warned = false;
  return function deprecatedWrapper(...args) {
    if (!warned) {
      warned = true;
      console.warn(msg);
    }
    return fn.apply(this, args);
  };
}

export function promisify(fn) {
  if (typeof fn !== 'function') throw new TypeError('fn must be a function');
  return function promisified(...args) {
    const self = this;
    return new Promise((resolve, reject) => {
      args.push(function callback(err, value) {
        if (err) reject(err);
        else resolve(value);
      });
      fn.apply(self, args);
    });
  };
}

export function callbackify(asyncFn) {
  if (typeof asyncFn !== 'function') throw new TypeError('asyncFn must be a function');
  return function callbackified(...args) {
    const cb = args.pop();
    if (typeof cb !== 'function') throw new TypeError('The last argument must be a function');
    asyncFn.apply(this, args).then(
      value => cb(null, value),
      reason => cb(reason || new Error('Promise was rejected with a falsy value'))
    );
  };
}

function isActualNaN(value) {
  return value !== value;
}

function sameValue(a, b) {
  if (isActualNaN(a) && isActualNaN(b)) return true;
  if (a === b) return a !== 0 || 1 / a === 1 / b;
  return false;
}

export function isDeepStrictEqual(a, b) {
  const seen = [];
  function eq(x, y) {
    if (sameValue(x, y)) return true;
    if (typeof x !== 'object' || x === null || typeof y !== 'object' || y === null) return false;
    if (seen.indexOf(x) >= 0) return true;
    if (Object.getPrototypeOf(x) !== Object.getPrototypeOf(y)) return false;
    seen.push(x);
    if (Array.isArray(x) || Array.isArray(y)) {
      if (!Array.isArray(x) || !Array.isArray(y) || x.length !== y.length) return false;
      for (let i = 0; i < x.length; i = i + 1) if (!eq(x[i], y[i])) return false;
      return true;
    }
    const xk = Object.keys(x);
    const yk = Object.keys(y);
    if (xk.length !== yk.length) return false;
    xk.sort();
    yk.sort();
    for (let i = 0; i < xk.length; i = i + 1) {
      if (xk[i] !== yk[i] || !eq(x[xk[i]], y[yk[i]])) return false;
    }
    return true;
  }
  return eq(a, b);
}

export const types = typesObject;

export const TextEncoder = globalThis.TextEncoder;
export const TextDecoder = globalThis.TextDecoder;
const utilDefault = {};
utilDefault.inherits = inherits;
utilDefault.promisify = promisify;
utilDefault.callbackify = callbackify;
utilDefault.format = format;
utilDefault.deprecate = deprecate;
utilDefault.inspect = inspect;
utilDefault.types = types;
utilDefault.isDeepStrictEqual = isDeepStrictEqual;
utilDefault.TextEncoder = TextEncoder;
utilDefault.TextDecoder = TextDecoder;
export default utilDefault;
