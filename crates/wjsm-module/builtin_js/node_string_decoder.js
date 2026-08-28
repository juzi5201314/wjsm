function normalizeEncoding(encoding) {
  if (encoding === undefined || encoding === null) return 'utf8';
  const lowered = String(encoding).toLowerCase();
  if (lowered === 'utf8' || lowered === 'utf-8') return 'utf8';
  if (lowered === 'utf16le' || lowered === 'utf-16le' || lowered === 'ucs2' || lowered === 'ucs-2') {
    return 'utf16le';
  }
  if (lowered === 'base64') return 'base64';
  if (lowered === 'latin1' || lowered === 'binary') return 'latin1';
  if (lowered === 'hex') return 'hex';
  if (lowered === 'ascii') return 'ascii';
  throw new TypeError('Unknown encoding: ' + String(encoding));
}

function toBuffer(input) {
  if (typeof input === 'string') {
    throw new TypeError('The "buf" argument must be an instance of Buffer, TypedArray, or DataView');
  }
  if (Buffer.isBuffer(input)) return input;
  if (ArrayBuffer.isView(input)) return Buffer.from(input);
  throw new TypeError('The "buf" argument must be an instance of Buffer, TypedArray, or DataView');
}

// 拆分组边界：utf16le 按 2 字节且缓冲末尾的高位代理项；base64 按 3 字节分组。
function pendingLength(encoding, buffer) {
  if (encoding === 'utf16le') {
    let keep = buffer.length % 2;
    const completeEnd = buffer.length - keep;
    if (completeEnd >= 2) {
      const unit = buffer[completeEnd - 2] + buffer[completeEnd - 1] * 256;
      if (unit >= 0xd800 && unit <= 0xdbff) keep = keep + 2;
    }
    return keep;
  }
  if (encoding === 'base64') return buffer.length % 3;
  return 0;
}

export class StringDecoder {
  constructor(encoding) {
    this.encoding = normalizeEncoding(encoding);
    if (this.encoding === 'utf8') {
      this._decoder = new TextDecoder('utf-8');
    } else {
      this._pending = null;
    }
  }

  write(buf) {
    const input = toBuffer(buf);
    if (this.encoding === 'utf8') {
      return this._decoder.decode(input, { stream: true });
    }
    if (this.encoding === 'utf16le' || this.encoding === 'base64') {
      const combined = this._pending === null ? input : Buffer.concat([this._pending, input]);
      const keep = pendingLength(this.encoding, combined);
      const emitEnd = combined.length - keep;
      this._pending = keep === 0 ? null : Buffer.from(combined.subarray(emitEnd, combined.length));
      if (emitEnd === 0) return '';
      return Buffer.from(combined.subarray(0, emitEnd)).toString(this.encoding);
    }
    return input.toString(this.encoding);
  }

  end(buf) {
    let out = buf === undefined ? '' : this.write(buf);
    if (this.encoding === 'utf8') {
      out = out + this._decoder.decode();
      return out;
    }
    if (this._pending !== null) {
      out = out + this._pending.toString(this.encoding);
      this._pending = null;
    }
    return out;
  }
}

export default { StringDecoder };
