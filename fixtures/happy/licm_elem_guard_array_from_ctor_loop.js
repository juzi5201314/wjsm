// Elem-guard：POINTS 由 Array.from + 统一类构造器填充，循环只读自有数据键。
// GuardElementsKind 应外提，POINTS[i].x 走模板槽。sum = 1+2+3 = 6。
class Point {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }
}
const POINTS = Array.from({ length: 3 }, (_, i) => new Point(i + 1, i + 2));
let sum = 0;
for (let i = 0; i < POINTS.length; i++) {
  sum += POINTS[i].x;
}
console.log(sum);
console.log(POINTS.length);
