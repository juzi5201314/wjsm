// IC 与原型链、GC 的交互：缓存只记录自有属性的位置，其余一律走宿主完整语义。

// 原型链上的属性不进缓存（本步只缓存自有属性），但值必须正确。
const base = { shared: 100 };
const child = Object.create(base);
child.own = 1;
let protoSum = 0;
for (let i = 0; i < 100; i++) protoSum += child.shared + child.own;
console.log("proto:", protoSum);

// 原型上的属性后改：即便读了很多次，也必须立刻反映新值（缓存不能缓存原型属性）。
base.shared = 200;
let afterSum = 0;
for (let i = 0; i < 10; i++) afterSum += child.shared;
console.log("proto-updated:", afterSum);

// 自有属性遮蔽原型属性：遮蔽后必须读到自有值。
child.shared = 5;
console.log("shadow:", child.shared, base.shared);
delete child.shared;
console.log("unshadow:", child.shared);

// 类实例：构造器建立的自有属性可缓存，原型方法调用走宿主。
class Point {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }
  sum() {
    return this.x + this.y;
  }
}
let clsSum = 0;
for (let i = 0; i < 300; i++) {
  const p = new Point(i, i + 1);
  clsSum += p.x + p.y + p.sum();
}
console.log("class:", clsSum);

// 改原型方法后，已缓存的自有属性读不受影响，方法调用必须用新实现。
Point.prototype.sum = function () {
  return this.x * this.y;
};
const p2 = new Point(3, 4);
console.log("proto-method:", p2.x, p2.y, p2.sum());

// GC 之后缓存仍必须正确：对象可能被搬迁，句柄状态转为非稳定态时快链退回宿主。
const kept = [];
for (let k = 0; k < 8; k++) kept.push({ tag: k, pad: "x" });
let gcSum = 0;
for (let round = 0; round < 3; round++) {
  // 制造分配压力触发 GC
  for (let i = 0; i < 30000; i++) {
    const garbage = { a: i, b: i + 1 };
    if (garbage.a < 0) console.log("unreachable");
  }
  for (const o of kept) gcSum += o.tag;
}
console.log("after-gc:", gcSum, kept.length, kept[7].tag);

// 大量不同「首属性名」的对象：根 shape 高扇出不得导致字典退化（IC 必须持续可用）。
const names = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"];
let fanout = 0;
for (const n of names) {
  const o = {};
  o[n] = 1;
  fanout += o[n];
}
console.log("fanout:", fanout);
