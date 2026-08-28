// this TDZ 合法用例：super() 前创建的箭头仅捕获 this 绑定，
// 实际读取发生在 super() 之后，不触发 TDZ。
class Base {
  constructor() {
    this.tag = "base";
  }
}
class Derived extends Base {
  constructor() {
    const late = () => this.tag;
    super();
    this.own = "derived";
    console.log(late(), this.own);
  }
}
new Derived();
