// 形参默认值中的 super()：参数绑定阶段即完成实例初始化，
// super() 表达式的值为绑定后的 this。
class Base {
  constructor() {
    this.base = 1;
  }
}
class Derived extends Base {
  constructor(a = super()) {
    console.log(a === this, this.base);
  }
}
new Derived();
