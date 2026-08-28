// super() 前访问 this 抛出的 ReferenceError 属于构造器体的求值，
// 可在体内捕获；捕获后再调用 super() 仍可正常完成构造。
class Base {
  constructor() {
    this.base = "ok";
  }
}
class Derived extends Base {
  constructor() {
    try {
      console.log(this.base);
    } catch (e) {
      console.log("caught:", e.constructor.name);
    }
    super();
    console.log("after:", this.base);
  }
}
new Derived();
