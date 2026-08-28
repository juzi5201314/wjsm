// Buffer.prototype 按 Node 形态物化：实例 → Buffer.prototype →
// %Uint8Array.prototype% → %TypedArray%.prototype 原型链、构造器静态链
// Buffer → Uint8Array、链上可变性与删除不复活。输出与 Node v22 逐字节一致。
const buf = Buffer.from('ab');

// 原型链身份
console.log(typeof Buffer.prototype);
console.log(Object.getPrototypeOf(buf) === Buffer.prototype);
console.log(Object.getPrototypeOf(Buffer.prototype) === Uint8Array.prototype);
console.log(buf instanceof Buffer, buf instanceof Uint8Array, buf instanceof Object);
console.log(Buffer.prototype.constructor === Buffer, buf.constructor === Buffer);
console.log(Object.getPrototypeOf(Buffer) === Uint8Array);
console.log(Buffer.name, Buffer.length, Buffer.BYTES_PER_ELEMENT);

// 构造器自有形态（读取 own keys 不再触发 InternalInvariant）
const dp = Object.getOwnPropertyDescriptor(Buffer, 'prototype');
console.log(dp.writable, dp.enumerable, dp.configurable);
const statics = Object.getOwnPropertyNames(Buffer);
console.log(
  statics.includes('length'),
  statics.includes('name'),
  statics.includes('prototype'),
  statics.includes('from'),
  statics.includes('alloc'),
);
const df = Object.getOwnPropertyDescriptor(Buffer, 'from');
console.log(df.writable, df.enumerable, df.configurable);
console.log(Buffer.from.name, Buffer.from.length, Buffer.alloc.name, Buffer.alloc.length);
console.log(Buffer.isBuffer.name, Buffer.isBuffer.length, Buffer.byteLength.length, Buffer.concat.length);

// Buffer.prototype 自有方法：constructor 居首，方法保持 Node 定义次序
const subset = [
  'readUInt32LE', 'readUInt16LE', 'readUInt8', 'readUInt32BE', 'readUInt16BE',
  'readInt32LE', 'readInt16LE', 'readInt8', 'readInt32BE', 'readInt16BE',
  'writeUInt32LE', 'writeUInt16LE', 'writeUInt8', 'writeUInt32BE', 'writeUInt16BE',
  'writeInt32LE', 'writeInt16LE', 'writeInt8', 'writeInt32BE', 'writeInt16BE',
  'readFloatLE', 'readFloatBE', 'readDoubleLE', 'readDoubleBE',
  'writeFloatLE', 'writeFloatBE', 'writeDoubleLE', 'writeDoubleBE',
  'copy', 'toString', 'equals', 'compare', 'indexOf', 'includes', 'fill',
  'write', 'toJSON', 'subarray', 'slice',
];
const own = Object.getOwnPropertyNames(Buffer.prototype);
console.log(own[0]);
console.log(own.filter((name) => subset.includes(name)).join(','));
const dm = Object.getOwnPropertyDescriptor(Buffer.prototype, 'toString');
console.log(dm.writable, dm.enumerable, dm.configurable);
const dc = Object.getOwnPropertyDescriptor(Buffer.prototype, 'constructor');
console.log(dc.writable, dc.enumerable, dc.configurable);
console.log(Buffer.prototype.toString.name, Buffer.prototype.toString.length);
console.log(Buffer.prototype.readDoubleLE.name, Buffer.prototype.writeFloatBE.name);

// Buffer 自有方法遮蔽 %TypedArray%.prototype 同名方法，TypedArray 方法沿链继承
console.log(buf.slice === Buffer.prototype.slice, buf.slice === Uint8Array.prototype.slice);
console.log(Uint8Array.prototype.slice.call(buf, 0, 1) instanceof Uint8Array);
console.log([...buf].join(','), buf.at(1), buf.map((byte) => byte + 1).join(','));
console.log(buf.slice(0, 1) instanceof Buffer, buf.subarray(1) instanceof Buffer);
console.log(`${buf}`, buf.toString('hex'));

// 链上可变性：覆盖 / 自定义方法 / 删除立即可见，删除不复活
Buffer.prototype.hello = function () {
  return `hello ${this.length}`;
};
console.log(buf.hello());
delete Buffer.prototype.hello;
console.log(typeof buf.hello);
Buffer.prototype.toString = function () {
  return 'patched';
};
console.log(`${buf}`, buf.toString());
delete Buffer.prototype.toString;
console.log(buf.toString === Object.prototype.toString, typeof buf.toString);
