// arguments.callee 取值恒为当前执行的函数对象（§10.4.4 CreateMappedArgumentsObject
// 的 func 实参）：覆盖具名声明/具名表达式/匿名表达式/generator/async 各形态，
// 以及体内不引用自身名字、体内含嵌套函数（历史上两条取值路径各自的盲区）。

// 具名函数声明：体内不引用 f，callee 仍须解析为 f。
function f() {
  return arguments.callee;
}
console.log("decl:", typeof f(), f() === f);

// 具名函数表达式：callee === 表达式求值产物。
var g = function g2() {
  return arguments.callee;
};
console.log("named expr:", typeof g(), g() === g);

// 匿名函数表达式（含嵌套函数，历史上 FunctionRef id 预测在此失准）。
var h = function () {
  var inner = function () {};
  return inner, arguments.callee;
};
console.log("anon expr:", typeof h(), h() === h);

// generator 声明：wrapper 帧物化 arguments，callee 指向用户可见的 gen。
function* gen() {
  yield arguments.callee;
}
console.log("gen decl:", typeof gen().next().value, gen().next().value === gen);

// generator 表达式。
var genExpr = function* gname() {
  yield arguments.callee;
};
console.log("gen expr:", genExpr().next().value === genExpr);

// 经 callee 递归（无捕获与有捕获两种帧形态）。
function fact(n) {
  return n < 2 ? 1 : n * arguments.callee(n - 1);
}
console.log("recursion:", fact(5));
let step = 3;
var rec = function (n) {
  return n === 0 ? 0 : step + arguments.callee(n - 1);
};
console.log("recursion w/ capture:", rec(3));

// 对象字面量方法（非严格 → mapped）。
var o = {
  m() {
    return arguments.callee;
  },
};
console.log("method:", o.m() === o.m);

// bind/call/apply 转发后 callee 仍为原函数。
function bf() {
  return arguments.callee;
}
console.log("bind:", bf.bind(null)() === bf);
console.log("call:", bf.call(null) === bf, "apply:", bf.apply(null) === bf);

// new 调用：构造帧的 callee 为构造器本身。
function Ctor() {
  this.callee = arguments.callee;
}
console.log("new:", new Ctor().callee === Ctor);

// 属性形状不因取值解析而改变：writable/不可枚举/configurable 数据属性。
function shape() {
  var d = Object.getOwnPropertyDescriptor(arguments, "callee");
  console.log("shape:", d.writable, d.enumerable, d.configurable, typeof d.value);
  console.log("keys:", Object.keys(arguments).join(","));
}
shape(10, 20);

// async 声明与 async generator 声明：wrapper 帧物化，callee 为用户可见函数。
async function af() {
  return arguments.callee;
}
async function* ag() {
  yield arguments.callee;
}
af()
  .then(function (v) {
    console.log("async decl:", v === af);
    return ag().next();
  })
  .then(function (r) {
    console.log("async gen decl:", r.value === ag);
  });
