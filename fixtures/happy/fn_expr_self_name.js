// 具名函数表达式自身名字绑定（§15.2.5 InstantiateOrdinaryFunctionExpression：
// funcEnv 上 CreateImmutableBinding(name, false) + InitializeBinding）：
// 体内自引用解析为该函数对象；名字对外不可见；funcEnv 按每次求值新建；
// 写入非严格静默忽略、严格抛运行时 TypeError。

// 基本递归与 typeof / identity。
var g = function g2(n) {
  return n > 0 ? g2(n - 1) : "done";
};
console.log("recursion:", g(3));
console.log("typeof inside:", (function f() { return typeof f; })());
console.log("identity:", (function f() { return f; })().name);
console.log("name invisible:", typeof g2);

// 自身名字与 arguments.callee 指向同一对象。
console.log("callee match:", (function f() { return f === arguments.callee; })());

// 形参默认值可引用自身名字（调用期 funcEnv 已初始化）。
console.log("default param:", (function f(a = f) { return a === f; })());

// 非严格写入静默忽略（RHS 副作用保留）；严格写入抛 TypeError（可捕获）。
var sideEffect = 0;
var sloppyWrite = (function f() {
  f = (sideEffect = 42);
  f += 1;
  f &&= 0;
  f++;
  return typeof f;
})();
console.log("sloppy writes:", sloppyWrite, sideEffect);
var strictWrite = (function f() {
  "use strict";
  try {
    f = 1;
    return "no-throw";
  } catch (e) {
    return "caught:" + (e instanceof TypeError);
  }
})();
console.log("strict write:", strictWrite);

// 解构与 for-in 头写入同样按不可变语义。
console.log("destructuring:", (function f() { [f] = [1]; return typeof f; })());
console.log("for-in head:", (function f() { for (f in { a: 1 }); return typeof f; })());

// delete 自身名字：声明式不可删除绑定返回 false（§9.1.1.1.8）。
console.log("delete:", (function f() { return delete f; })());

// 内层 var / 形参遮蔽自身名字。
console.log("var shadow:", (function f() { var f = 1; return f; })());
console.log("param shadow:", (function f(f) { return f; })(9));

// funcEnv 按每次求值新建：循环内每轮闭包持有独立的自身名字绑定，
// 递归穿过自身名字仍读到该轮的 let 捕获。
var fns = [];
for (let i = 0; i < 3; i++) {
  fns.push(function named(n) {
    return n > 0 ? named(n - 1) : i;
  });
}
console.log("loop capture:", fns.map(function (f) { return f(1); }).join(","));
var pair = [];
for (let i = 0; i < 2; i++) {
  pair.push(function named() {
    return named;
  });
}
console.log("loop identity:", pair[0]() === pair[0], pair[0]() === pair[1]());

// 嵌套具名表达式：内层同时看见两层名字。
var outerFn = function outer() {
  return function inner() {
    return [typeof outer, typeof inner].join(",");
  };
};
console.log("nested:", outerFn()());

// 经 new 的递归（自身名字在 [[Construct]] 路径同样可解析）。
var Ctor = function C(n) {
  this.v = n > 0 ? new C(n - 1).v + 1 : 0;
};
console.log("new recursion:", new Ctor(2).v);

// generator 表达式：yield* 经自身名字递归。
var gen = function* rec(n) {
  if (n > 0) yield* rec(n - 1);
  yield n;
};
console.log("generator:", [...gen(2)].join(","));
console.log("generator name invisible:", typeof rec);

// 非严格 eval 写入静默忽略、读可见。
console.log("eval:", (function f() {
  eval("f = 1");
  return eval("typeof f");
})());

// async / async generator 表达式的自引用（顺序输出收尾）。
var asyncFn = async function af(n) {
  return n > 0 ? await af(n - 1) : "async-base";
};
var asyncGen = async function* ag(n) {
  yield typeof ag;
  yield n;
};
(async function () {
  console.log("async:", await asyncFn(2));
  var collected = [];
  for await (const v of asyncGen(7)) collected.push(v);
  console.log("async gen:", collected.join(","));
})();
