// 类 getter 内联 Math.hypot(this.x, this.y)：快路径与覆盖后的慢路径。
class Point {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }
  get norm() {
    return Math.hypot(this.x, this.y);
  }
}

const p = new Point(3, 4);
console.log(p.norm);
const q = new Point("3", "4");
console.log(q.norm);
Math.hypot = () => 0;
console.log(p.norm);
