// 二次 super()：父构造器先执行（[[Construct]] 每次新建 thisArgument，
// 副作用落在随后被 BindThisValue 拒绝丢弃的新对象上），再抛可捕获的
// ReferenceError；首个 super() 绑定的 this 不受影响（n 仍为 1）。
class Base {
  constructor() {
    this.n = (this.n | 0) + 1;
  }
}
class Derived extends Base {
  constructor() {
    super();
    try {
      super();
    } catch (e) {
      console.log("caught:", e.constructor.name, "-", e.message);
    }
    console.log("n:", this.n);
  }
}
new Derived();
