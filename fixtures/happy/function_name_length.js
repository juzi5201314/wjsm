// 函数 name/length 对齐 Node：类构造器 / new.target / 方法 / 访问器 /
// bound 函数（§10.2.9 SetFunctionName、§10.2.10 SetFunctionLength、
// §10.4.1.3 BoundFunctionCreate 后的 length/name 初始化）。
class C {
  constructor(a, b) {
    if (new.target) console.log("nt:", new.target.name);
  }
  m(a) {}
  get x() {
    return 1;
  }
  set x(v) {}
  static sm(a, b, c) {}
  ["comp" + "uted"](a, b) {}
}
new C();
console.log(C.name, C.length);
console.log(C.prototype.m.name, C.prototype.m.length);
const xd = Object.getOwnPropertyDescriptor(C.prototype, "x");
console.log(xd.get.name, xd.get.length, xd.set.name, xd.set.length);
console.log(C.sm.name, C.sm.length);
console.log(C.prototype.computed.name, C.prototype.computed.length);

// 派生类缺省构造器：name 取类名，length 为 0（...args 是 rest）。
class D extends C {}
new D();
console.log(D.name, D.length);

// bound：name 为 "bound " + 目标名，length 为目标 length 减已绑实参（下限 0），链式叠加。
function f(a, b, c) {}
const b1 = f.bind(null, 1);
const b2 = b1.bind(null, 2, 3);
console.log(b1.name, b1.length);
console.log(b2.name, b2.length);
console.log(C.bind(null).name, C.bind(null, 1).length);

// 声明 / 表达式 / 箭头 / generator / async：length 按 ExpectedArgumentCount
// （默认值与 rest 之后的形参不计入）。
function decl(a, b = 1, ...rest) {}
console.log(decl.name, decl.length);
const g = function* (a) {};
const ag = async function (a, b) {};
const agen = async function* () {};
console.log(g.name, g.length, ag.name, ag.length, agen.name, agen.length);
const named = function inner() {};
console.log(named.name);
