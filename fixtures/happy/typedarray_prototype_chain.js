// TypedArray 实例原型链（§23.2）：实例 → Ctor.prototype → %TypedArray%.prototype
// → %Object.prototype%；length / byteLength / byteOffset 是共享原型上的规范
// accessor，getter 可跨元素类型复用，品牌检查失败按 V8 口径抛 TypeError。

const bytes = new Uint8Array([1, 2, 3]);

// 三层链
console.log("proto: " + (Object.getPrototypeOf(bytes) === Uint8Array.prototype));
const shared = Object.getPrototypeOf(Uint8Array.prototype);
console.log("shared: " + (shared === Object.getPrototypeOf(Int32Array.prototype)) + " " + (shared === Object.getPrototypeOf(BigInt64Array.prototype)));
console.log("tail: " + (Object.getPrototypeOf(shared) === Object.prototype) + " " + (shared === Object.prototype));

// 每种构造器的实例链
console.log("kinds: " + [Int8Array, Uint8ClampedArray, Int16Array, Uint16Array, Uint32Array, Float32Array, Float64Array].every((Ctor) => Object.getPrototypeOf(new Ctor(1)) === Ctor.prototype));

// 派生实例保持链
console.log("subarray: " + (Object.getPrototypeOf(bytes.subarray(1)) === Uint8Array.prototype));
console.log("slice: " + (Object.getPrototypeOf(bytes.slice(0, 1)) === Uint8Array.prototype));
console.log("buffer view: " + (Object.getPrototypeOf(new Uint32Array(new ArrayBuffer(8))) === Uint32Array.prototype));
console.log("clone: " + (Object.getPrototypeOf(structuredClone(new Float64Array(1))) === Float64Array.prototype));
console.log("encode: " + (Object.getPrototypeOf(new TextEncoder().encode("hi")) === Uint8Array.prototype));

// instanceof 沿链成立
console.log("instanceof: " + (bytes instanceof Uint8Array) + " " + (bytes instanceof Object) + " " + (bytes instanceof Int8Array));

// 方法在共享原型上自有，Ctor.prototype 仅 constructor 与 BYTES_PER_ELEMENT
console.log("own: " + Object.getOwnPropertyNames(Uint8Array.prototype).join(","));
console.log("has slice: " + shared.hasOwnProperty("slice") + " " + Uint8Array.prototype.hasOwnProperty("slice") + " " + ("slice" in Uint8Array.prototype));
console.log("identity: " + (bytes.slice === Uint8Array.prototype.slice) + " " + (Uint8Array.prototype.slice === Int32Array.prototype.slice));

// BYTES_PER_ELEMENT：构造器与原型双侧，三特性全 false
const bpe = Object.getOwnPropertyDescriptor(Uint8Array.prototype, "BYTES_PER_ELEMENT");
console.log("bpe: " + bpe.value + " " + bpe.writable + " " + bpe.enumerable + " " + bpe.configurable);
console.log("bpe statics: " + Uint8Array.BYTES_PER_ELEMENT + " " + Int16Array.BYTES_PER_ELEMENT + " " + Float64Array.BYTES_PER_ELEMENT + " " + BigUint64Array.prototype.BYTES_PER_ELEMENT);

// length / byteLength / byteOffset 是共享原型上的 accessor
for (const name of ["length", "byteLength", "byteOffset"]) {
  const descriptor = Object.getOwnPropertyDescriptor(shared, name);
  console.log(name + ": " + descriptor.get.name + " " + descriptor.get.length + " " + descriptor.set + " " + descriptor.enumerable + " " + descriptor.configurable);
  console.log(name + " own: " + Object.getOwnPropertyDescriptor(Uint8Array.prototype, name) + " " + Object.getOwnPropertyDescriptor(bytes, name));
}

// getter 跨元素类型复用与实例读一致
const getByteLength = Object.getOwnPropertyDescriptor(shared, "byteLength").get;
const wide = new Float64Array(new ArrayBuffer(32), 8, 2);
console.log("get: " + getByteLength.call(bytes) + " " + getByteLength.call(wide) + " " + wide.length + " " + wide.byteLength + " " + wide.byteOffset);
console.log("in: " + ("length" in bytes) + " " + ("byteOffset" in bytes));

// 品牌检查失败：V8 口径 TypeError
try {
  getByteLength.call({});
} catch (error) {
  console.log("plain: " + (error instanceof TypeError) + " " + error.message);
}
try {
  Uint8Array.prototype.length;
} catch (error) {
  console.log("proto read: " + (error instanceof TypeError) + " " + error.message);
}
try {
  getByteLength.call(null);
} catch (error) {
  console.log("null: " + error.message);
}
try {
  getByteLength.call(7);
} catch (error) {
  console.log("number: " + error.message);
}
