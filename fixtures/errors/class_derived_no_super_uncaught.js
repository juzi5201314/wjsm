// [[Construct]] 步骤 15：派生构造器体正常完结而 super() 未执行时，
// GetThisBinding 读到未初始化的 this 抛 ReferenceError。该异常属于
// [[Construct]]（体完结之后），体内 try/catch 不可捕获。
class Base {
  constructor() {}
}
class Derived extends Base {
  constructor() {
    if (false) super();
  }
}
new Derived();
console.log("unreachable");
