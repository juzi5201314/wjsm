// 调用/构造实参位置的异常传播语义（ECMAScript ArgumentListEvaluation /
// EvaluateNew）：实参、receiver、callee 求值抛错必须中止调用并传播，
// 不得把 TAG_EXCEPTION 当作实参值传入（曾打印 [object Object]）。
function boom() {
  throw new Error("boom");
}
function id(x) {
  return x;
}

// 普通调用实参抛错 → 传播
try {
  id(boom());
  console.log("FAIL plain arg");
} catch (e) {
  console.log("plain arg:", e.message);
}

// 多层嵌套实参：外层调用不得执行
try {
  id(id(boom()));
  console.log("FAIL nested arg");
} catch (e) {
  console.log("nested arg:", e.message);
}

// 求值顺序：抛错实参之前的实参恰好求值一次，之后的不再求值
const order = [];
function rec(x) {
  order.push(x);
  return x;
}
try {
  id(rec(1), boom(), rec(3));
  console.log("FAIL order");
} catch (e) {
  order.push("caught");
}
console.log("order:", order.join(","));

// 方法调用实参抛错 → 方法不得执行
const obj = {
  m(x) {
    console.log("FAIL method body ran");
    return x;
  },
};
try {
  obj.m(boom());
} catch (e) {
  console.log("method arg:", e.message);
}

// receiver 求值抛错 → 属性访问与调用都不得发生
function getReceiver() {
  throw new Error("receiver");
}
try {
  getReceiver().m(1);
  console.log("FAIL receiver");
} catch (e) {
  console.log("receiver:", e.message);
}

// 可选调用实参抛错 → 传播（receiver 非空时实参照常求值）
try {
  obj?.m(boom());
  console.log("FAIL optional call arg");
} catch (e) {
  console.log("optional call arg:", e.message);
}

// new 实参抛错 → 构造器不得执行
class C {
  constructor(x) {
    console.log("FAIL ctor ran");
    this.x = x;
  }
}
try {
  new C(boom());
} catch (e) {
  console.log("new arg:", e.message);
}

// super() 实参抛错 → 基类构造器不得执行
class Base {
  constructor(x) {
    console.log("FAIL base ctor ran");
    this.x = x;
  }
}
class Derived extends Base {
  constructor() {
    super(boom());
  }
}
try {
  new Derived();
} catch (e) {
  console.log("super arg:", e.message);
}

// 宿主 builtin 实参抛错 → 传播（console.log 主复现 / JSON / Math）
try {
  console.log(boom());
} catch (e) {
  console.log("console arg:", e.message);
}
try {
  JSON.stringify(boom());
  console.log("FAIL JSON.stringify arg");
} catch (e) {
  console.log("stringify arg:", e.message);
}
const mathOrder = [];
function mrec(x) {
  mathOrder.push(x);
  return x;
}
try {
  Math.max(mrec(1), boom(), mrec(3));
  console.log("FAIL Math.max arg");
} catch (e) {
  mathOrder.push("caught");
}
console.log("math order:", mathOrder.join(","));

// 字符串原型方法实参抛错 → 传播
try {
  "abcdef".slice(boom());
  console.log("FAIL slice arg");
} catch (e) {
  console.log("slice arg:", e.message);
}

// 模板字面量插值抛错：作为实参时同样传播
try {
  id(`x${boom()}y`);
  console.log("FAIL template arg");
} catch (e) {
  console.log("template arg:", e.message);
}

// 标签模板插值抛错 → tag 函数不得执行
function tag(strings, value) {
  console.log("FAIL tag ran");
  return value;
}
try {
  tag`x${boom()}y`;
} catch (e) {
  console.log("tagged template:", e.message);
}

// 逻辑/条件表达式作为实参：左/分支抛错传播且不误入右侧
try {
  id(boom() && rec("FAIL rhs"));
  console.log("FAIL logical arg");
} catch (e) {
  console.log("logical arg:", e.message);
}
try {
  id(true ? boom() : rec("FAIL alt"));
  console.log("FAIL cond arg");
} catch (e) {
  console.log("cond arg:", e.message);
}

// 正常调用不受影响
function twice(x) {
  return x * 2;
}
console.log("ok:", id(7), twice(21), `t${id(1)}`, JSON.stringify([2, 3]));
console.log("done");
