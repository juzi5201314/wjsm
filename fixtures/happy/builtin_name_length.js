// 内建函数 JS 可见 name/length（各函数小节的固有 name 与 §17 的 length
// 约定），数值与 Node 对齐；Math/Array/String/Object/Reflect/JSON/Promise/
// 集合/生成器/全局函数逐类抽查。
console.log(Math.max.name, Math.max.length);
console.log(Math.random.name, Math.random.length);
console.log(Math.atan2.name, Math.atan2.length);
console.log(Array.prototype.map.name, Array.prototype.map.length);
console.log([].splice.name, [].splice.length);
console.log([].values.name, [].values.length);
console.log("".split.name, "".split.length);
console.log("".replace.name, "".replace.length);
console.log(""[Symbol.iterator].name, ""[Symbol.iterator].length);
console.log(Object.defineProperty.name, Object.defineProperty.length);
console.log(Object.getOwnPropertyDescriptor.name, Object.getOwnPropertyDescriptor.length);
console.log(Object.prototype.hasOwnProperty.name, Object.prototype.hasOwnProperty.length);
console.log(Reflect.set.name, Reflect.set.length);
console.log(Reflect.apply.name, Reflect.apply.length);
console.log(JSON.parse.name, JSON.parse.length);
console.log(JSON.stringify.name, JSON.stringify.length);
console.log(Promise.resolve.name, Promise.all.length, Promise.withResolvers.name);
console.log(Date.now.name, Date.now.length, Date.UTC.name, Date.UTC.length);
console.log(Number.name, Number.length, Boolean.name, Symbol.name, BigInt.name);
console.log(Error.name, Error.length, TypeError.name, TypeError.length);
console.log(Map.name, Map.length, Set.name, WeakMap.name, WeakRef.name, WeakRef.length);
console.log(Proxy.name, Proxy.length);
console.log(parseInt.name, parseInt.length, parseFloat.name, parseFloat.length);
console.log(isNaN.name, isNaN.length, isFinite.name, isFinite.length);
console.log(Number.isInteger.name, Number.parseInt.length, Symbol.for.name);
console.log(Function.prototype.call.name, Function.prototype.call.length);
console.log(Function.prototype.apply.name, Function.prototype.apply.length);
console.log(Function.prototype.bind.name, Function.prototype.bind.length);
console.log(Function.prototype.toString.name, Function.prototype.toString.length);
console.log(console.log.name, console.log.length);

function checkInstances() {
  const m = new Map();
  console.log(m.set.name, m.set.length, m.get.name, m.keys.name, m.forEach.length);
  const s = new Set();
  console.log(s.add.name, s.keys.name, s.values.name, s.entries.name);
  const wm = new WeakMap();
  console.log(wm.set.name, wm.set.length, wm.has.name);
  const ta = new Int8Array(1);
  console.log(ta.map.name, ta.subarray.length, ta.set.length, Int8Array.name, Int8Array.length);
  const dv = new DataView(new ArrayBuffer(8));
  console.log(dv.getInt32.name, dv.getInt32.length, dv.setFloat64.name, dv.setFloat64.length);
  const ab = new ArrayBuffer(4);
  console.log(ab.slice.name, ab.slice.length, ArrayBuffer.name, DataView.name);
  console.log(/x/[Symbol.match].name, /x/[Symbol.replace].length);
  console.log((1).toFixed.name, (1).toString.length, true.toString.name);
  console.log(Symbol.prototype.toString.name, Symbol.prototype.valueOf.name);
}
checkInstances();

function checkGenerators() {
  function* g() {}
  const it = g();
  console.log(it.next.name, it.next.length, it.return.name, it.throw.name);
}
checkGenerators();
