// [[Construct]] 步骤 13.b：派生构造器显式 return 非对象且非 undefined 的
// 原语时抛 TypeError；该检查发生在构造器体完结之后（[[Construct]] 层），
// 未捕获则终止执行，`new` 之后的代码不得执行。
class Base {
  constructor() {
    return { marker: 1 };
  }
}
class Derived extends Base {
  constructor() {
    super();
    console.log("before-return");
    return 5;
  }
}
new Derived();
console.log("unreachable after new");
