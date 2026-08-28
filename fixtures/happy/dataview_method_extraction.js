// DataView 原型方法可取值 + BigInt64/BigUint64 get/set 全族。

const view = new DataView(new ArrayBuffer(16));

// 实例与 DataView.prototype 上 get/set 全族均为函数
const names = [
  "getInt8", "getUint8", "getInt16", "getUint16", "getInt32", "getUint32",
  "getFloat32", "getFloat64", "getBigInt64", "getBigUint64",
  "setInt8", "setUint8", "setInt16", "setUint16", "setInt32", "setUint32",
  "setFloat32", "setFloat64", "setBigInt64", "setBigUint64",
];
console.log("typeof: " + names.every(function (name) {
  return typeof view[name] === "function" && typeof DataView.prototype[name] === "function";
}));

// 取值后经 call / bind / apply 使用
view.setUint8.call(view, 0, 255);
const readUint8 = view.getUint8;
console.log("call: " + readUint8.call(view, 0));
console.log("bind: " + view.getUint8.bind(view)(0));
console.log("apply: " + DataView.prototype.getUint8.apply(view, [0]));

// name / length 元数据与 constructor
console.log("meta: " + readUint8.name + " " + readUint8.length + " " + DataView.prototype.setFloat64.name + " " + DataView.prototype.setFloat64.length);
console.log("constructor: " + (DataView.prototype.constructor === DataView));

// BigInt64 / BigUint64 全族（含字节序与位型重解释）
view.setBigInt64(0, -2n);
view.setBigUint64(8, 18446744073709551615n, true);
console.log("bigint: " + view.getBigInt64(0) + " " + view.getBigUint64(0) + " " + view.getBigInt64(8, true) + " " + view.getBigUint64(8, true));

// 取值后的 setBigUint64 按 2^64 取模写入
const setBig = view.setBigUint64;
setBig.call(view, 0, (1n << 65n) | 7n);
console.log("wrap: " + view.getBigUint64(0));

// setBigInt64 非 BigInt 输入按 ToBigInt 抛 TypeError
try {
  view.setBigInt64(0, 1);
} catch (error) {
  console.log("type error: " + (error instanceof TypeError));
}
