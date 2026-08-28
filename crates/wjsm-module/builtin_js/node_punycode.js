// RFC 3492 Punycode + RFC 3490 IDNA 分段转换（对应 Node 弃用的 punycode 模块）。
const maxInt = 2147483647;
const base = 36;
const tMin = 1;
const tMax = 26;
const skew = 38;
const damp = 700;
const initialBias = 72;
const initialN = 128;
const delimiter = '-';

const regexPunycode = /^xn--/;
const regexNonASCII = /[^\x00-\x7F]/;
const regexSeparators = /[\x2E\u3002\uFF0E\uFF61]/g;

function overflowError() {
  return new RangeError('Overflow: input needs wider integers to process');
}

function ucs2decode(string) {
  const output = [];
  const text = String(string);
  let counter = 0;
  const length = text.length;
  while (counter < length) {
    const value = text.charCodeAt(counter);
    counter = counter + 1;
    if (value >= 0xd800 && value <= 0xdbff && counter < length) {
      const extra = text.charCodeAt(counter);
      if ((extra & 0xfc00) === 0xdc00) {
        output.push(((value & 0x3ff) << 10) + (extra & 0x3ff) + 0x10000);
        counter = counter + 1;
      } else {
        output.push(value);
      }
    } else {
      output.push(value);
    }
  }
  return output;
}

function ucs2encode(codePoints) {
  let out = '';
  for (let i = 0; i < codePoints.length; i = i + 1) {
    out = out + String.fromCodePoint(codePoints[i]);
  }
  return out;
}

function basicToDigit(codePoint) {
  if (codePoint >= 0x30 && codePoint < 0x3a) return 26 + (codePoint - 0x30);
  if (codePoint >= 0x41 && codePoint < 0x5b) return codePoint - 0x41;
  if (codePoint >= 0x61 && codePoint < 0x7b) return codePoint - 0x61;
  return base;
}

function digitToBasic(digit, flag) {
  return digit + 22 + 75 * (digit < 26 ? 1 : 0) - ((flag !== 0 ? 1 : 0) << 5);
}

function adapt(delta, numPoints, firstTime) {
  let k = 0;
  delta = firstTime ? Math.floor(delta / damp) : delta >> 1;
  delta = delta + Math.floor(delta / numPoints);
  while (delta > ((base - tMin) * tMax) >> 1) {
    delta = Math.floor(delta / (base - tMin));
    k = k + base;
  }
  return Math.floor(k + ((base - tMin + 1) * delta) / (delta + skew));
}

export function decode(input) {
  const output = [];
  const text = String(input);
  const inputLength = text.length;
  let i = 0;
  let n = initialN;
  let bias = initialBias;

  let basic = text.lastIndexOf(delimiter);
  if (basic < 0) basic = 0;
  for (let j = 0; j < basic; j = j + 1) {
    const code = text.charCodeAt(j);
    if (code >= 0x80) throw new RangeError('Illegal input >= 0x80 (not a basic code point)');
    output.push(code);
  }

  let index = basic > 0 ? basic + 1 : 0;
  while (index < inputLength) {
    const oldi = i;
    let w = 1;
    let k = base;
    while (true) {
      if (index >= inputLength) throw new RangeError('Invalid input');
      const digit = basicToDigit(text.charCodeAt(index));
      index = index + 1;
      if (digit >= base) throw new RangeError('Invalid input');
      if (digit > Math.floor((maxInt - i) / w)) throw overflowError();
      i = i + digit * w;
      const t = k <= bias ? tMin : k >= bias + tMax ? tMax : k - bias;
      if (digit < t) break;
      const baseMinusT = base - t;
      if (w > Math.floor(maxInt / baseMinusT)) throw overflowError();
      w = w * baseMinusT;
      k = k + base;
    }
    const out = output.length + 1;
    bias = adapt(i - oldi, out, oldi === 0);
    if (Math.floor(i / out) > maxInt - n) throw overflowError();
    n = n + Math.floor(i / out);
    i = i % out;
    output.splice(i, 0, n);
    i = i + 1;
  }

  return ucs2encode(output);
}

export function encode(input) {
  const codePoints = ucs2decode(String(input));
  const inputLength = codePoints.length;
  let output = '';
  let n = initialN;
  let delta = 0;
  let bias = initialBias;

  for (let j = 0; j < inputLength; j = j + 1) {
    const value = codePoints[j];
    if (value < 0x80) output = output + String.fromCharCode(value);
  }

  const basicLength = output.length;
  let handledCPCount = basicLength;
  if (basicLength > 0) output = output + delimiter;

  while (handledCPCount < inputLength) {
    let m = maxInt;
    for (let j = 0; j < inputLength; j = j + 1) {
      const value = codePoints[j];
      if (value >= n && value < m) m = value;
    }
    const handledCPCountPlusOne = handledCPCount + 1;
    if (m - n > Math.floor((maxInt - delta) / handledCPCountPlusOne)) throw overflowError();
    delta = delta + (m - n) * handledCPCountPlusOne;
    n = m;
    for (let j = 0; j < inputLength; j = j + 1) {
      const value = codePoints[j];
      if (value < n) {
        delta = delta + 1;
        if (delta > maxInt) throw overflowError();
      }
      if (value === n) {
        let q = delta;
        let k = base;
        while (true) {
          const t = k <= bias ? tMin : k >= bias + tMax ? tMax : k - bias;
          if (q < t) break;
          output = output + String.fromCharCode(digitToBasic(t + ((q - t) % (base - t)), 0));
          q = Math.floor((q - t) / (base - t));
          k = k + base;
        }
        output = output + String.fromCharCode(digitToBasic(q, 0));
        bias = adapt(delta, handledCPCountPlusOne, handledCPCount === basicLength);
        delta = 0;
        handledCPCount = handledCPCount + 1;
      }
    }
    delta = delta + 1;
    n = n + 1;
  }

  return output;
}

// 分离邮箱本地部与域名，逐 label 应用转换（Node punycode.toASCII/toUnicode 同款策略）。
function mapDomain(domain, callback) {
  const text = String(domain);
  const parts = text.split('@');
  let result = '';
  let rest = text;
  if (parts.length > 1) {
    result = parts[0] + '@';
    rest = parts[1];
  }
  const labels = rest.replace(regexSeparators, '.').split('.');
  const encoded = [];
  for (let i = 0; i < labels.length; i = i + 1) {
    encoded.push(callback(labels[i]));
  }
  return result + encoded.join('.');
}

export function toASCII(input) {
  return mapDomain(input, label =>
    regexNonASCII.test(label) ? 'xn--' + encode(label) : label
  );
}

export function toUnicode(input) {
  return mapDomain(input, label =>
    regexPunycode.test(label) ? decode(label.slice(4).toLowerCase()) : label
  );
}

export const ucs2 = { decode: ucs2decode, encode: ucs2encode };
export const version = '2.1.0';

export default { version, ucs2, decode, encode, toASCII, toUnicode };
