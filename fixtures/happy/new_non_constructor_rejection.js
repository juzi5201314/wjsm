// IsConstructor（§7.2.4）：无 [[Construct]] 的 native callable 在 construct
// 调用一律拒绝 TypeError "<expr> is not a constructor"；Symbol / BigInt 有
// [[Construct]]（extends / newTarget 合法）但构造期自抛（§20.4.1.1 /
// §21.2.1.1 步骤 1）。文案与 Node 对拍。
function probe(label, fn) {
  try {
    fn();
    console.log(label, "ok");
  } catch (e) {
    console.log(label, e.constructor.name + ": " + e.message);
  }
}
probe("Math.max", () => new Math.max());
probe("JSON.parse", () => new JSON.parse("{}"));
probe("parseInt", () => new parseInt("1"));
probe("array values", () => new [].map(x => x));
probe("console.log", () => new console.log());
probe("Symbol", () => new Symbol());
probe("BigInt", () => new BigInt(1));
probe("Function.prototype", () => new Function.prototype());
probe("string iterator", () => new ""[Symbol.iterator]());
probe("bound non-constructor", () => new (Math.max.bind(null))());
probe("bound constructor", () => {
  const C = function () {};
  new (C.bind(null))();
});
probe("Reflect.construct", () => Reflect.construct(Math.max, []));
probe("proxy non-constructor target", () => new (new Proxy(Math.max, {}))());
probe("proxy constructor target", () => new (new Proxy(function () {}, {}))());
// 构造器不受影响；Symbol 作 extends 值与 newTarget 合法（IsConstructor true）。
probe("Map", () => new Map());
probe("Promise", () => new Promise(r => r(1)));
probe("RegExp", () => new RegExp("x"));
probe("extends Symbol", () => { class X extends Symbol {} });
probe("newTarget Symbol", () => { Reflect.construct(function () {}, [], Symbol); });
// BigInt 字面量与调用形式不受构造检查影响（含构造器体内的直连站点）。
class Wrapper {
  constructor() {
    this.big = 7n;
    this.sym = Symbol("in-ctor");
  }
}
const wrapper = new Wrapper();
console.log(typeof wrapper.big, typeof wrapper.sym, BigInt(5) === 5n, typeof Symbol("x"));
