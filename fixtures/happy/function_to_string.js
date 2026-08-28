// Function.prototype.toString（§20.2.3.5）：有 [[SourceText]] 的用户函数
// 返回原始源码片段（含默认值/rest/注释/换行的精确切片）；内建/bound/
// Function.prototype 返回 NativeFunction 形态；非 callable this 抛 TypeError。
function decl(a, b = 1, ...rest) { return a; }
const expr = function named(x) { return x; };
const arrow = (a, b) => a + b;
async function af(x) { return x; }
function* gf(y) { yield y; }
async function* agf(z) { yield z; }
console.log(decl.toString());
console.log(expr.toString());
console.log(arrow.toString());
console.log(af.toString());
console.log(gf.toString());
console.log(agf.toString());

// 类：类本体、方法、static 方法（toString 剥离 static 前缀）、访问器。
class C {
  constructor(a) { this.a = a; }
  m(x) { return x; }
  static s() {}
  get g() { return 1; }
  set g(v) {}
}
console.log(C.toString());
console.log(C.prototype.m.toString());
console.log(C.s.toString());
const gd = Object.getOwnPropertyDescriptor(C.prototype, "g");
console.log(gd.get.toString());
console.log(gd.set.toString());

// 对象字面量方法与访问器。
const obj = { m(x) { return x; }, get p() { return 1; }, set p(v) {} };
console.log(obj.m.toString());
const pd = Object.getOwnPropertyDescriptor(obj, "p");
console.log(pd.get.toString());
console.log(pd.set.toString());

// 内建 / bound / Function.prototype：NativeFunction 形态。
console.log(Function.prototype.toString.call(Math.max));
console.log(Array.prototype.map.toString());
console.log(decl.bind(null, 1).toString());
console.log(Function.prototype.toString.call(Function.prototype));

// 非 callable this：TypeError。
try {
  Function.prototype.toString.call(1);
} catch (e) {
  console.log(e instanceof TypeError);
}
try {
  Function.prototype.toString.call({});
} catch (e) {
  console.log(e instanceof TypeError);
}
