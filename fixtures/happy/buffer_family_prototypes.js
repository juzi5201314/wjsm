// buffer 家族原型对收口：ArrayBuffer.prototype / SharedArrayBuffer.prototype
// 物化为真实堆对象，DataView 实例挂 DataView.prototype，constructor /
// @@toStringTag / 品牌一致；buffer / byteLength / byteOffset 为规范
// accessor getter。stdout 与 Node v22 逐字节一致。

const ab = new ArrayBuffer(16);
const sab = new SharedArrayBuffer(8);
const dv = new DataView(ab, 4, 8);

// 实例 [[Prototype]] === 构造器.prototype，instanceof / constructor 成对。
console.log(
  Object.getPrototypeOf(ab) === ArrayBuffer.prototype,
  Object.getPrototypeOf(sab) === SharedArrayBuffer.prototype,
  Object.getPrototypeOf(dv) === DataView.prototype,
);
console.log(ab instanceof ArrayBuffer, sab instanceof SharedArrayBuffer, dv instanceof DataView);
console.log(
  ab.constructor === ArrayBuffer,
  sab.constructor === SharedArrayBuffer,
  dv.constructor === DataView,
);
console.log(
  ArrayBuffer.prototype.constructor === ArrayBuffer,
  SharedArrayBuffer.prototype.constructor === SharedArrayBuffer,
  DataView.prototype.constructor === DataView,
);

// @@toStringTag 品牌：Object.prototype.toString 经原型链取得。
console.log(Object.prototype.toString.call(ab));
console.log(Object.prototype.toString.call(sab));
console.log(Object.prototype.toString.call(dv));
const tag = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, Symbol.toStringTag);
console.log(tag.value, tag.writable, tag.enumerable, tag.configurable);
console.log(SharedArrayBuffer.prototype[Symbol.toStringTag], DataView.prototype[Symbol.toStringTag]);

// 构造器 `prototype` 自有属性三特性全 false（§25.1.5.2 / §25.2.5.1 / §25.3.3.1）。
for (const ctor of [ArrayBuffer, SharedArrayBuffer, DataView]) {
  const d = Object.getOwnPropertyDescriptor(ctor, "prototype");
  console.log(ctor.name, d.writable, d.enumerable, d.configurable);
}

// DataView buffer / byteLength / byteOffset 是原型上的规范 accessor getter。
for (const name of ["buffer", "byteLength", "byteOffset"]) {
  const d = Object.getOwnPropertyDescriptor(DataView.prototype, name);
  console.log(name, typeof d.get, d.set, d.enumerable, d.configurable, d.get.name, d.get.length);
}
console.log(dv.buffer === ab, dv.byteLength, dv.byteOffset);

// ArrayBuffer / SharedArrayBuffer 访问器同为原型上的规范 getter。
const abLen = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, "byteLength");
console.log(typeof abLen.get, abLen.set, abLen.get.name, abLen.get.length);
for (const name of ["byteLength", "growable", "maxByteLength"]) {
  const d = Object.getOwnPropertyDescriptor(SharedArrayBuffer.prototype, name);
  console.log(name, typeof d.get, d.get.name);
}

// getter 取出后经 call 使用（this 为携带品牌的实例）。
console.log(abLen.get.call(ab), Object.getOwnPropertyDescriptor(DataView.prototype, "byteOffset").get.call(dv));

// 原型方法可取值复用，slice 结果保持品牌。
console.log(typeof ArrayBuffer.prototype.slice, typeof SharedArrayBuffer.prototype.slice, typeof SharedArrayBuffer.prototype.grow);
const abSlice = ab.slice(0, 4);
const sabSlice = sab.slice(0, 4);
console.log(abSlice instanceof ArrayBuffer, abSlice.byteLength, sabSlice instanceof SharedArrayBuffer, sabSlice.byteLength);

// 构造器无实参：ToIndex(undefined) = 0（§25.1.4.1 / §25.2.4.1）。
console.log(new ArrayBuffer().byteLength, new SharedArrayBuffer().byteLength);

// @@species 访问器（§25.1.5.3 / §25.2.5.2）。
console.log(ArrayBuffer[Symbol.species] === ArrayBuffer, SharedArrayBuffer[Symbol.species] === SharedArrayBuffer);

// TypedArray 的 buffer 与 Buffer 底层 buffer 均为携带品牌的 ArrayBuffer。
const u8 = new Uint8Array(4);
console.log(u8.buffer instanceof ArrayBuffer, Object.prototype.toString.call(u8.buffer));

// structuredClone 保持三者品牌。
const clone = structuredClone({ ab, dv, sab });
console.log(clone.ab instanceof ArrayBuffer, clone.dv instanceof DataView, clone.sab instanceof SharedArrayBuffer);
console.log(clone.dv.byteOffset, clone.dv.byteLength, clone.dv.buffer === clone.ab);
