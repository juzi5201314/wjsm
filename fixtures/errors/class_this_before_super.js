// this TDZ（ES §9.1.1.3.4）：派生构造器中 super() 之前访问 this 是
// 运行时 ReferenceError（GetThisBinding 读到未初始化绑定），而非
// 静态早错误——延迟引用（箭头）与形参默认值 super() 等程序合法。
class Base {
  constructor() {
    this.base = true;
  }
}

class Derived extends Base {
  constructor() {
    this.x = 1;
    super();
  }
}

new Derived();
