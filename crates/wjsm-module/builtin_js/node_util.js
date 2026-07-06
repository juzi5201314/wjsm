
function callFunction(fn, receiver, args) {
  if (args.length === 0) return fn.call(receiver);
  if (args.length === 1) return fn.call(receiver, args[0]);
  if (args.length === 2) return fn.call(receiver, args[0], args[1]);
  if (args.length === 3) return fn.call(receiver, args[0], args[1], args[2]);
  if (args.length === 4) return fn.call(receiver, args[0], args[1], args[2], args[3]);
  if (args.length === 5) return fn.call(receiver, args[0], args[1], args[2], args[3], args[4]);
  return fn.call(receiver, args[0], args[1], args[2], args[3], args[4], args[5]);
}

function copyEnumerableProperties(target, source) {
  if (!source || typeof source !== 'object') return;
  const keys = Object.keys(source);
  for (let i = 0; i < keys.length; i = i + 1) target[keys[i]] = source[keys[i]];
}

function collectDefinedArgs(a, b, c, d, e, f) {
  const args = [];
  if (a !== undefined) args.push(a);
  if (b !== undefined) args.push(b);
  if (c !== undefined) args.push(c);
  if (d !== undefined) args.push(d);
  if (e !== undefined) args.push(e);
  if (f !== undefined) args.push(f);
  return args;
}

export function inherits(constructor, superConstructor) {
  if (typeof constructor !== 'function' || typeof superConstructor !== 'function') {
    throw new TypeError('The "constructor" and "superConstructor" arguments must be functions');
  }
  constructor.super_ = superConstructor;
  const proto = {};
  copyEnumerableProperties(proto, superConstructor.prototype);
  proto.constructor = constructor;
  constructor.prototype = proto;

  const marker = '__wjsm_inherits_' + (constructor.name || 'anonymous');
  const originalCall = superConstructor.call;
  superConstructor.call = function inheritedSuperCall(receiver, a, b, c, d, e, f) {
    const result = callFunction(originalCall, superConstructor, [receiver].concat(collectDefinedArgs(a, b, c, d, e, f)));
    if (receiver && (typeof receiver === 'object' || typeof receiver === 'function')) {
      receiver[marker] = true;
      copyEnumerableProperties(receiver, superConstructor.prototype);
    }
    return result;
  };

  const previousHasInstance = superConstructor[Symbol.hasInstance];
  superConstructor[Symbol.hasInstance] = function inheritedHasInstance(obj) {
    if (obj && obj[marker]) return true;
    if (obj && obj.constructor === constructor) return true;
    if (typeof previousHasInstance === 'function') return previousHasInstance.call(this, obj);
    return false;
  };
}

export function inspect(obj, opts) {
  const depth = opts && opts.depth !== undefined ? opts.depth : 2;
  const seen = [];
  function inner(value, level) {
    if (value === null) return 'null';
    if (typeof value === 'string') return "'" + value + "'";
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

export function format(fmt, a, b, c, d, e, f) {
  const args = collectDefinedArgs(a, b, c, d, e, f);
  if (typeof fmt !== 'string') {
    const values = [fmt].concat(args);
    return values.map(v => inspect(v)).join(' ');
  }
  let index = 0;
  let out = '';
  for (let i = 0; i < fmt.length; i = i + 1) {
    if (fmt.charAt(i) !== '%' || i + 1 >= fmt.length) {
      out = out + fmt.charAt(i);
      continue;
    }
    const code = fmt.charAt(i + 1);
    i = i + 1;
    if (code === '%') { out = out + '%'; continue; }
    if (index >= args.length) { out = out + '%' + code; continue; }
    const arg = args[index];
    index = index + 1;
    if (code === 's') out = out + String(arg);
    else if (code === 'd' || code === 'i') out = out + parseInt(arg, 10);
    else if (code === 'f') out = out + Number(arg);
    else if (code === 'j') {
      try { out = out + JSON.stringify(arg); } catch (err) { out = out + '[Circular]'; }
    } else if (code === 'o' || code === 'O') out = out + inspect(arg);
    else out = out + '%' + code;
  }
  while (index < args.length) {
    out = out + ' ' + inspect(args[index]);
    index = index + 1;
  }
  return out;
}

export function deprecate(fn, msg) {
  let warned = false;
  return function deprecatedWrapper(a, b, c, d, e, f) {
    if (!warned) {
      warned = true;
      console.warn(msg);
    }
    return callFunction(fn, this, collectDefinedArgs(a, b, c, d, e, f));
  };
}

export function promisify(fn) {
  if (typeof fn !== 'function') throw new TypeError('fn must be a function');
  return function promisified(a, b, c, d, e, f) {
    const self = this;
    const args = collectDefinedArgs(a, b, c, d, e, f);
    return new Promise((resolve, reject) => {
      args.push(function callback(err, value) {
        if (err) reject(err);
        else resolve(value);
      });
      callFunction(fn, self, args);
    });
  };
}

export function callbackify(asyncFn) {
  if (typeof asyncFn !== 'function') throw new TypeError('asyncFn must be a function');
  return function callbackified(a, b, c, d, e, f) {
    const args = collectDefinedArgs(a, b, c, d, e, f);
    const cb = args.pop();
    if (typeof cb !== 'function') throw new TypeError('The last argument must be a function');
    callFunction(asyncFn, this, args).then(
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

export const types = {
  isDate: value => value instanceof Date,
  isRegExp: value => value instanceof RegExp,
  isMap: value => value instanceof Map,
  isSet: value => value instanceof Set,
  isPromise: value => !!value && typeof value.then === 'function',
  isProxy: () => false
};

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
