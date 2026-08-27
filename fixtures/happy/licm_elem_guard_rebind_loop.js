// Elem-guard 语义回归：循环体内替换 POINTS[1]，SetElem 不在白名单 →
// 静态放弃守卫。第二次迭代必须读到新元素：sum = 1 + 100 + 3 = 104。
const POINTS = [{ x: 1 }, { x: 2 }, { x: 3 }];
let sum = 0;
for (let i = 0; i < POINTS.length; i++) {
  sum = sum + POINTS[i].x;
  if (i === 0) {
    POINTS[1] = { x: 100 };
  }
}
console.log(sum);
