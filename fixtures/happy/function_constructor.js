// Function 动态函数构造器（§20.2.1.1）：调用/构造形式、参数拼接、
// ToString 强制转换、全局作用域（非词法捕获）、name/length/prototype。
// 全部输出与 Node 一致（node fixtures/happy/function_constructor.js 校验）。

// 基本形式：new Function 与 Function(...) 等价。
const add = new Function("a", "return a + 1");
console.log(add(2));
console.log(Function("return 40 + 2")());
console.log(String(new Function()()));
console.log(new Function("return 7")());

// 参数拼接：逗号分组、默认值、rest、解构、行注释。
console.log(new Function("a, b", "return a + b")(3, 4));
console.log(new Function("a = 5", "return a")());
console.log(new Function("...rest", "return rest.length")(1, 2, 3));
console.log(new Function("{x, y}", "return x * y")({ x: 6, y: 7 }));
console.log(new Function("a // comment", "return a")(5));
console.log(new Function("a = (1, 2)", "return a")());
console.log(new Function("...[x, y]", "return x + y")(3, 4));

// 实参 ToString 强制转换（含对象 toString 与求值顺序）。
console.log(new Function({ toString() { return "n"; } }, { toString() { return "return n * 2"; } })(21));
console.log(String(new Function(0)()));
const order = [];
new Function(
  { toString() { order.push("p1"); return "a"; } },
  { toString() { order.push("p2"); return "b"; } },
  { toString() { order.push("body"); return "return 1"; } }
);
console.log(order.join());

// 作用域是全局环境：不捕获定义处的函数词法作用域。
function outerScope() {
  var local = 99;
  return new Function("return typeof local")();
}
console.log(outerScope());
globalThis.dynGlobal = 123;
console.log(new Function("return dynGlobal")());
new Function("globalThis.dynSet = 55")();
console.log(globalThis.dynSet);

// 函数体拥有独立的变量环境：var 与函数声明不泄漏到全局。
console.log(new Function("var v = 1; (function(){ v = 2 })(); return v")());
new Function("var leaked = 77; function inner() {}")();
console.log(typeof globalThis.leaked, typeof globalThis.inner);

// name 为 "anonymous"（函数体内不存在该绑定）；length 为 ExpectedArgumentCount。
console.log(new Function("").name);
console.log(new Function("return typeof anonymous")());
console.log(new Function("a", "b", "return 1").length);
console.log(new Function("a", "...b", "return 1").length);
console.log(new Function("a", "b = 1", "c", "return 1").length);
console.log(new Function("{a}", "b", "return a + b").length);
const named = new Function("");
try {
  named.name = "zzz";
} catch (ignored) {}
console.log(named.name);

// prototype：新鲜 prototype 对象、constructor 回链、可构造、instanceof。
const ctor = new Function("this.x = 9");
console.log(typeof ctor.prototype, ctor.prototype.constructor === ctor);
const instance = new ctor();
console.log(instance.x, Object.getPrototypeOf(instance) === ctor.prototype, instance instanceof ctor);
console.log(new Function("") instanceof Function);
console.log(new Function("") instanceof Object);
console.log(Object.getPrototypeOf(new Function("")) === Function.prototype);

// Function 构造器自身的元数据。
console.log(Function.name, Function.length);
console.log(typeof Function.prototype);
console.log(String(Function.prototype()));
console.log(new Function("return Function")() === Function);

// 重复形参（sloppy 简单参数列表）：后者胜，arguments 仍按位置。
console.log(new Function("a", "a", "return a")(7, 8));
console.log(new Function("a", "a", "return arguments[0] + arguments[1]")(7, 8));

// arguments 对象与 new.target。
console.log(new Function("return arguments.length")(1, 2, 3));
console.log(new Function("return new.target === undefined")());
const Cls = new Function("return typeof new.target");
console.log(Cls());
console.log(typeof new Cls());
console.log(new Function("const a = () => new.target; return a() === undefined")());

// "use strict" 指令生效于函数体。
console.log(new Function("'use strict'; return typeof this")());

// 嵌套闭包：动态函数体内定义的函数正常捕获体内绑定。
console.log(new Function("x", "return function(y){ return x + y }")(1)(2));
