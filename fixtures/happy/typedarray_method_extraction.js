// TypedArray 原型方法可取值：方法引用经 call/bind/apply 复用，
// Constructor.prototype 上的方法与实例取值为同一函数。

const bytes = new Uint8Array([1, 2, 3]);

// 取值后 typeof 与 call
const sub = bytes.subarray;
console.log("typeof: " + typeof sub);
console.log("call: " + sub.call(bytes, 1).length);

// bind / apply
console.log("bind: " + sub.bind(bytes)(2).length);
console.log("apply: " + bytes.slice.apply(bytes, [0, 2]).join(","));

// Constructor.prototype 路径
console.log("proto slice: " + Uint8Array.prototype.slice.call(bytes, 0, 1).join(","));
console.log("proto typeof: " + typeof Int32Array.prototype.map + " " + typeof Float64Array.prototype.sort);
console.log("constructor: " + (Uint8Array.prototype.constructor === Uint8Array));

// name / length 元数据
console.log("meta: " + sub.name + " " + sub.length + " " + Uint8Array.prototype.sort.name + " " + Uint8Array.prototype.sort.length);

// Reflect.get 与解构取到同一函数
console.log("reflect: " + (Reflect.get(bytes, "subarray") === bytes.subarray));
const { slice } = bytes;
console.log("destructure: " + (slice === Uint8Array.prototype.slice) + " " + slice.call(bytes, 1).join(","));

// 原型对象的 @@iterator 与 in
console.log("iterator: " + [...Uint8Array.prototype[Symbol.iterator].call(bytes)].join(","));
console.log("in: " + ("subarray" in Uint8Array.prototype) + " " + ("slice" in Uint8Array.prototype));
console.log("source: " + String(bytes.subarray));
