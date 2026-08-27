// 未捕获的隐式 super() 异常必须终止执行：派生类缺省构造器的
// `? Construct(func, args, NewTarget)` 抛出后，字段初始化器与
// `new` 之后的代码都不得执行。
class Base {
  constructor() {
    console.log("base-before-throw");
    throw new Error("implicit-super-boom");
  }
}
class Derived extends Base {
  unreached = console.log("unreachable field");
}
new Derived();
console.log("unreachable after new");
