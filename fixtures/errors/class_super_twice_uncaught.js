// 二次 super() 未捕获：规范顺序为 Construct 先执行（父构造器副作用
// 可见，"base-run" 打印两次），BindThisValue 检测到 this 已初始化后
// 抛 ReferenceError 终止执行。
class Base {
  constructor() {
    console.log("base-run");
  }
}
class Derived extends Base {
  constructor() {
    super();
    super();
  }
}
new Derived();
console.log("unreachable");
