// ArrayBuffer resizable 家族（ES2024 §25.1）：构造器 maxByteLength 选项、
// resizable / maxByteLength / detached 三个规范 accessor getter、resize 的
// 原地 grow（补零）/ shrink（截断）语义与 V8 口径错误路径。
// stdout 与 Node v22 逐字节一致。

// 原型形态：成员齐全且顺序对齐 V8，方法可写可配置不可枚举，getter 规范特性。
console.log(Object.getOwnPropertyNames(ArrayBuffer.prototype).join(","));
console.log(ArrayBuffer.prototype.resize.name, ArrayBuffer.prototype.resize.length);
console.log(ArrayBuffer.prototype.transfer.name, ArrayBuffer.prototype.transfer.length);
console.log(
  ArrayBuffer.prototype.transferToFixedLength.name,
  ArrayBuffer.prototype.transferToFixedLength.length,
);
for (const name of ["resizable", "maxByteLength", "detached"]) {
  const d = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, name);
  console.log(name, typeof d.get, d.set, d.enumerable, d.configurable, d.get.name, d.get.length);
}
const rd = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, "resize");
console.log(rd.writable, rd.enumerable, rd.configurable);

// 构造：options.maxByteLength 经 ToIndex，undefined / 非对象保持固定长度。
const rab = new ArrayBuffer(4, { maxByteLength: 16 });
console.log(rab.resizable, rab.maxByteLength, rab.byteLength, rab.detached);
const fixed = new ArrayBuffer(6);
console.log(fixed.resizable, fixed.maxByteLength, fixed.detached);
console.log(new ArrayBuffer(4, { maxByteLength: "12" }).maxByteLength);
console.log(new ArrayBuffer(4, { maxByteLength: undefined }).resizable, new ArrayBuffer(4, "str").resizable);

// resize：grow 补零、shrink 截断、resize() 即 ToIndex(undefined) = 0。
const bytes = new Uint8Array(rab);
bytes[3] = 7;
rab.resize(8);
console.log(rab.byteLength, bytes[3], bytes[7]);
rab.resize(2);
console.log(rab.byteLength);
rab.resize("6");
console.log(rab.byteLength, bytes[3]);
rab.resize();
console.log(rab.byteLength);
rab.resize(16);
console.log(rab.byteLength);

// getter 取出经 call 使用（品牌在实例侧表）。
const g = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, "resizable").get;
console.log(g.call(rab), g.call(fixed));

// 错误路径：固定长度 / 非 AB receiver 的品牌检查、越界与负值 RangeError。
try { fixed.resize(4); } catch (e) { console.log(e.constructor.name, e.message); }
try { ArrayBuffer.prototype.resize.call({}, 4); } catch (e) { console.log(e.constructor.name, e.message); }
try { g.call({}); } catch (e) { console.log(e.constructor.name, e.message); }
try { rab.resize(17); } catch (e) { console.log(e.constructor.name, e.message); }
try { rab.resize(-1); } catch (e) { console.log(e.constructor.name, e.message); }
try { new ArrayBuffer(8, { maxByteLength: 4 }); } catch (e) { console.log(e.constructor.name, e.message); }
try { new ArrayBuffer(8, { maxByteLength: -1 }); } catch (e) { console.log(e.constructor.name, e.message); }

// slice 结果收敛为固定长度（§25.1.6.16）。
const sliced = rab.slice(0, 4);
console.log(sliced.byteLength, sliced.resizable, sliced.maxByteLength);
