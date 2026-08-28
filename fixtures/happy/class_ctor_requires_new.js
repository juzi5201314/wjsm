// 类构造器 [[Call]] 拒绝（ES §10.2.1 步骤 2）：不带 new 的直接调用、
// call/apply/bind、回调、Reflect.apply、Proxy 转发均抛 TypeError；
// [[Construct]]（new / Reflect.construct / bound construct）不受影响。
// 文案与 Node/V8 对齐：命名类含类名，匿名类用复数句式。

function message(run) {
  try {
    run();
    return "no error";
  } catch (error) {
    return `${error.constructor.name}: ${error.message}`;
  }
}

class C {
  constructor(a, b) {
    this.sum = (a ?? 0) + (b ?? 0);
  }
}

// 直接调用与 call/apply。
console.log(message(() => C()));
console.log(message(() => C.call(null)));
console.log(message(() => C.apply(null, [])));

// bound 的 [[Call]] 仍拒绝（目标类构造器名保留）。
const B = C.bind(null, 1);
console.log(message(() => B()));

// 回调路径（宿主 invoke funnel）。
console.log(message(() => [1].map(C)));

// Reflect.apply 是 [[Call]]，Proxy apply 转发到类构造器目标同样拒绝。
console.log(message(() => Reflect.apply(C, null, [])));
console.log(message(() => new Proxy(C, {})()));

// 派生类与匿名/命名类表达式的文案。
class D extends C {}
console.log(message(() => D()));
console.log(message(() => (class {})()));
const A = class Inner {};
console.log(message(() => A()));

// [[Construct]] 正常：new / Reflect.construct / bound construct。
console.log(new C(1, 2).sum, new C() instanceof C);
console.log(Reflect.construct(C, [2, 3]).sum);
const constructed = new B(2);
console.log(constructed.sum, constructed instanceof C);

// bound 链逐层解包：prototype 解析到最终目标，this 初始化保留。
const B2 = B.bind(null);
const viaChain = new B2(4);
console.log(viaChain.sum, viaChain instanceof C);

// 派生类 new 正常，原型链完整。
const derived = new D(5, 6);
console.log(derived.sum, derived instanceof D, derived instanceof C);

// Reflect.construct 显式 newTarget 穿过 bound：原型取自 newTarget。
function X() {}
X.prototype.tag = "x";
const reflected = Reflect.construct(C.bind(null, 7), [8], X);
console.log(reflected.sum, reflected.tag, reflected instanceof X);

// 普通函数 bound construct 的 new.target：SameValue(bound, newTarget) 时
// 替换为目标（ES §10.4.1.2 步骤 5）。
function G() {
  return { hit: new.target === G };
}
console.log(new (G.bind(null))().hit);
const BG = G.bind(null);
console.log(BG().hit);

// 拒绝路径可重复触发且不破坏后续构造。
let rejections = 0;
for (let i = 0; i < 3; i++) {
  try {
    C();
  } catch (error) {
    if (error instanceof TypeError) rejections += 1;
  }
}
console.log(rejections, new C(9, 10).sum);
