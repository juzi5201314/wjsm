// 派生类缺省构造器等价 `constructor(...args) { super(...args); }`：
// 父构造器抛出的异常必须终止派生构造器并向 `new` 调用点传播，
// 可被外层 try/catch 捕获，不得被丢弃后当作构造成功。
class ThrowBase {
  constructor() {
    throw new Error("implicit-x");
  }
}
class ImplicitDerived extends ThrowBase {}
try {
  new ImplicitDerived();
  console.log("unreachable after new");
} catch (e) {
  console.log("caught-implicit:", e.message);
}

// super() 抛出后实例字段初始化器不得执行（字段初始化在 super() 成功之后）。
class FieldDerived extends ThrowBase {
  x = (console.log("unreachable field init"), 1);
}
try {
  new FieldDerived();
} catch (e) {
  console.log("caught-field:", e.message);
}

// 缺省构造器实参转发（...args）在正常路径保持不变。
class SumBase {
  constructor(a, b) {
    this.sum = a + b;
  }
}
class SumDerived extends SumBase {}
console.log("forwarded:", new SumDerived(40, 2).sum);

// 多级继承：中间层缺省构造器逐层转发异常。
class MidDerived extends ThrowBase {}
class LeafDerived extends MidDerived {
  constructor() {
    super();
    console.log("unreachable after super");
  }
}
try {
  new LeafDerived();
} catch (e) {
  console.log("caught-chain:", e.message);
}

// 显式 super() 与 super(...spread) 的异常同样传播（回归守卫）。
class ExplicitDerived extends ThrowBase {
  constructor() {
    super();
    console.log("unreachable after explicit super");
  }
}
try {
  new ExplicitDerived();
} catch (e) {
  console.log("caught-explicit:", e.message);
}
class SpreadDerived extends ThrowBase {
  constructor() {
    super(...[1, 2]);
    console.log("unreachable after spread super");
  }
}
try {
  new SpreadDerived();
} catch (e) {
  console.log("caught-spread:", e.message);
}
console.log("done");
