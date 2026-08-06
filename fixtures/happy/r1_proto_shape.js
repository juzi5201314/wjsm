// proto 仍在对象头 +0，但换 proto / 原型长属性都必须让下游解析立刻反映出来。
const base = { greet: "hi" };
const child = Object.create(base);
child.own = 1;
console.log(child.own, child.greet, Object.keys(child).join(","));

// 原型后加属性 → 子对象立刻可见。
base.added = "later";
console.log(child.added);

// 遮蔽：自有属性优先于原型。
child.greet = "own-hi";
console.log(child.greet, base.greet);
delete child.greet;
console.log(child.greet);

// 换 proto。
const other = { greet: "other-hi", extra: 2 };
Object.setPrototypeOf(child, other);
console.log(child.greet, child.extra, child.added, child.own);

// null proto。
Object.setPrototypeOf(child, null);
console.log(child.own, child.greet, child.extra);

// class 原型上的方法（原型链数据属性的主路径）。
class Point {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }
  sum() {
    return this.x + this.y;
  }
}
class Point3 extends Point {
  constructor(x, y, z) {
    super(x, y);
    this.z = z;
  }
  sum() {
    return super.sum() + this.z;
  }
}
const p = new Point(1, 2);
const p3 = new Point3(1, 2, 3);
console.log(p.sum(), p3.sum());
console.log(Object.keys(p).join(","), Object.keys(p3).join(","));

// 同类实例共享 shape：多次构造后读写全对。
let total = 0;
for (let i = 0; i < 100; i++) {
  total += new Point(i, i + 1).sum();
}
console.log(total);

// 改原型方法 → 已有实例的调用结果必须跟着变（原型链缓存须失效）。
Point.prototype.sum = function () {
  return this.x * this.y;
};
console.log(p.sum(), new Point(3, 4).sum());
