// NamedEvaluation（ES §8.4.5）：匿名函数定义按绑定上下文命名——变量声明、
// 赋值（含逻辑赋值）、形参 / 解构默认值、对象字面量属性、类字段。
const v = () => {};
let w;
w = function () {};
let z = null;
z ??= (a) => {};
let y = false;
y ||= function () {};
console.log(v.name, w.name, z.name, z.length, y.name);

function withDefault(cb = () => {}, { g = function () {} } = {}, [h = () => {}] = []) {
  console.log(cb.name, g.name, h.name);
}
withDefault();

// 对象字面量：静态键 / 数字键 / 字符串键 / 计算键 / symbol 键 / 访问器 / 方法。
const s = Symbol("tag");
const plain = Symbol();
const o = {
  kv: function () {},
  arrow: (a, b) => {},
  m(a) {},
  get acc() {
    return 1;
  },
  set acc(v) {},
  123: () => {},
  "str key": function () {},
  ["dyn" + "amic"]: () => {},
  [s]: function () {},
  [plain]: () => {},
};
console.log(o.kv.name, o.arrow.name, o.arrow.length, o.m.name);
const accD = Object.getOwnPropertyDescriptor(o, "acc");
console.log(accD.get.name, accD.set.name);
console.log(o[123].name, o["str key"].name, o.dynamic.name);
console.log(JSON.stringify(o[s].name), JSON.stringify(o[plain].name));

// 类字段：公有 / 私有（含 #）/ 静态 / 计算键（键求值后运行时命名）。
const key = "compField";
class K {
  pub = () => {};
  #priv = function () {};
  static st = (a) => {};
  static ["computed" + "St"] = function () {};
  [key] = () => {};
  names() {
    return [this.pub.name, this.#priv.name, this[key].name];
  }
}
const k = new K();
console.log(k.names().join(" "));
console.log(K.st.name, K.st.length, K.computedSt.name);

// 匿名类表达式命名、括号透传、非绑定位置保持空串。
const Klass = class {};
const paren = (function () {});
console.log(Klass.name, paren.name, JSON.stringify((() => {}).name));
