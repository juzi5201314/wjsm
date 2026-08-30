// 循环前改写 Point.prototype.norm：实例 shape 不变，原型 accessor 身份
// 失配 → GuardElementsKind 失败，POINTS[i].norm 走原 GetProp，读到 100。
class Point {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }
  get norm() {
    return Math.hypot(this.x, this.y);
  }
}
const POINTS = Array.from({ length: 2 }, () => new Point(3, 4));
Object.defineProperty(Point.prototype, "norm", {
  get() {
    return 100;
  },
});
let sum = 0;
for (let i = 0; i < POINTS.length; i++) {
  sum += POINTS[i].norm;
}
console.log(sum);
