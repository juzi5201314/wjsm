// 静态字段初始化器的 this 绑定（ClassDefinitionEvaluation）：
// 静态字段初始化器以合成函数求值，this 即构造器本身，与 static block 一致。
class Counter {
  static base = 40;
  static value = this.base + 2;
  static self = this;
}
console.log(Counter.value);
console.log(Counter.self === Counter);

// 箭头函数词法捕获初始化器的 this。
class Arrow {
  static nums = [1, 2, 3];
  static total = (() => this.nums.reduce((a, b) => a + b, 0))();
}
console.log(Arrow.total);

// 静态私有方法在静态字段初始化器运行前已绑定到构造器，this.#m() 可调用。
class Priv {
  static #seed = 7;
  static #double() { return this.#seed * 2; }
  static result = this.#double();
}
console.log(Priv.result);

// 私有静态字段初始化器同样以构造器为 this；静态私有访问器经 this 解析。
class PrivField {
  static width = 3;
  static #area = this.width * this.width;
  static #half() { return PrivField.#area / 2; }
  static getArea() { return PrivField.#area; }
  static getHalf() { return this.#half(); }
}
console.log(PrivField.getArea());
console.log(PrivField.getHalf());

// 静态字段初始化器与 static block 按源顺序交错执行，共享构造器 this。
class Mixed {
  static log = [];
  static a = (this.log.push("field-a"), "a");
  static { this.log.push("block"); }
  static b = (this.log.push("field-b"), "b");
}
console.log(Mixed.log.join(","));
console.log(Mixed.a, Mixed.b);

// 类表达式与计算键静态字段同样以构造器为 this。
const Expr = class {
  static v = 5;
  static w = this.v * 2;
};
console.log(Expr.w);

const key = "dyn";
class Computed {
  static [key + "1"] = this.name === undefined ? "no-name" : "has-name";
  static [key + "2"] = typeof this;
}
console.log(Computed.dyn2);

// 每次类求值创建独立构造器，初始化器的 this 跟随本次求值的构造器。
function make(n) {
  return class {
    static id = n;
    static twice = this.id * 2;
  };
}
console.log(make(3).twice, make(5).twice);

// new.target 在静态字段初始化器内为 undefined（普通调用，非构造）。
class Meta {
  static nt = new.target;
}
console.log(Meta.nt);

// 无初始化器的静态字段直接定义 undefined，且不影响后续初始化器的 this。
class Bare {
  static empty;
  static after = this.empty === undefined ? "ok" : "bad";
}
console.log(Bare.after);
