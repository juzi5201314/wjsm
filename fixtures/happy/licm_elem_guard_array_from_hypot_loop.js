// Elem-guard：Array.from + 类 hypot getter。pre-header 校验 shape 与
// Point.prototype.norm 仍是 hypot getter 后，POINTS[i].norm 走双槽 hypot。
// 三点 (3,4) 的 hypot 均为 5，sum = 15。
class Point {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }
  get norm() {
    return Math.hypot(this.x, this.y);
  }
}
const POINTS = Array.from({ length: 3 }, () => new Point(3, 4));
let sum = 0;
for (let i = 0; i < POINTS.length; i++) {
  sum += POINTS[i].norm;
}
console.log(sum);
