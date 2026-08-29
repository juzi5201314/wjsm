// SharedArrayBuffer 构造器与 Atomics 命名空间是全局对象上真实的自有数据
// 属性（急切物化，复用 FIX-04 路径）：own descriptor 可见、赋值 / 删除 /
// 重定义按普通属性语义生效，裸标识符读取按全局环境记录语义解析（删除后
// ReferenceError，typeof 容忍）。stdout 与 Node v22 逐字节一致。

// own descriptor：{writable, enumerable: false, configurable}。
for (const name of ["SharedArrayBuffer", "Atomics"]) {
  void globalThis[name];
  const d = Object.getOwnPropertyDescriptor(globalThis, name);
  console.log(name, typeof d.value, d.writable, d.enumerable, d.configurable);
}
console.log(Object.getOwnPropertyNames(globalThis).includes("SharedArrayBuffer"));
console.log(Reflect.ownKeys(globalThis).includes("Atomics"));
console.log(globalThis.hasOwnProperty("SharedArrayBuffer"), Object.hasOwn(globalThis, "Atomics"));
console.log(globalThis.propertyIsEnumerable("SharedArrayBuffer"), globalThis.propertyIsEnumerable("Atomics"));

// Atomics 命名空间对象：@@toStringTag 品牌，方法为可写可配置不可枚举的
// 自有数据属性，Object.keys 不见任何键。
console.log(Object.prototype.toString.call(Atomics), Atomics[Symbol.toStringTag]);
console.log(Object.keys(Atomics).length);
const add = Object.getOwnPropertyDescriptor(Atomics, "add");
console.log(typeof add.value, add.writable, add.enumerable, add.configurable, add.value.name, add.value.length);

// Atomics 静态方法快路径与覆盖后的通用路径。
const ta = new Int32Array(new SharedArrayBuffer(8));
Atomics.store(ta, 0, 5);
console.log(Atomics.load(ta, 0), Atomics.add(ta, 0, 2), Atomics.load(ta, 0));
const savedAdd = Atomics.add;
Atomics.add = () => "patched";
console.log(Atomics.add(ta, 0, 1), Atomics.load(ta, 0));
Atomics.add = savedAdd;
console.log(Atomics.add(ta, 0, 1), Atomics.load(ta, 0));

// 赋值替换 SharedArrayBuffer：裸读与 new 都用新值，赋回后恢复构造语义。
const savedSAB = SharedArrayBuffer;
globalThis.SharedArrayBuffer = function Fake(n) {
  this.n = n;
};
console.log(typeof SharedArrayBuffer, new SharedArrayBuffer(3).n, SharedArrayBuffer.name);
globalThis.SharedArrayBuffer = savedSAB;
console.log(SharedArrayBuffer === savedSAB, new SharedArrayBuffer(4).byteLength);

// defineProperty 访问器：读取走 getter，裸标识符读取同样生效。
const savedAtomics = Atomics;
Object.defineProperty(globalThis, "Atomics", {
  get() {
    return "from-getter";
  },
  configurable: true,
});
console.log(globalThis.Atomics, Atomics);
Object.defineProperty(globalThis, "Atomics", {
  value: savedAtomics,
  writable: true,
  configurable: true,
});
console.log(Atomics === savedAtomics, Atomics.load(ta, 0));

// 删除：typeof 容忍返回 undefined，裸标识符读取与 new 抛 ReferenceError。
console.log(delete globalThis.Atomics, typeof Atomics, "Atomics" in globalThis);
try {
  Atomics;
} catch (error) {
  console.log(error.constructor.name, error.message);
}
console.log(delete globalThis.SharedArrayBuffer, typeof SharedArrayBuffer);
try {
  SharedArrayBuffer;
} catch (error) {
  console.log(error.constructor.name, error.message);
}
try {
  new SharedArrayBuffer(1);
} catch (error) {
  console.log(error.constructor.name, error.message);
}

// 重新赋回后恢复完整构造语义（身份与 instanceof 不变）。
globalThis.SharedArrayBuffer = savedSAB;
console.log(new SharedArrayBuffer(2) instanceof SharedArrayBuffer);
