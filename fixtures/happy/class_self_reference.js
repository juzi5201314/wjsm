// 类体内延迟执行的函数体（构造器/方法/静态方法/实例字段/命名类表达式方法）可引用类名；
// 静态块/静态字段值/计算键/extends 在类求值期间执行，类名仍为 TDZ（见 errors__tdz）。
class Point {
  constructor(x, y) {
    this.x = x;
    this.y = y;
    this.origin = Point; // 实例字段初始化器引用类名
  }
  reflect() {
    return new Point(this.x, -this.y); // 方法体引用类名
  }
  static fromArray(arr) {
    return new Point(arr[0], arr[1]); // 静态方法引用类名
  }
}
const C = class Named {
  make() {
    return new Named(); // 命名类表达式方法引用类名
  }
};
const p = Point.fromArray([3, 4]);
console.log(p.reflect().x, p.reflect().y);
console.log(p.origin === Point);
console.log(new C().make() instanceof C);
