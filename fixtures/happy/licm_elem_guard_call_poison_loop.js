// Elem-guard 语义回归：循环体内调用用户函数（可任意改写元素 shape），
// Call 不在白名单 → 静态放弃守卫。第 3 次迭代必须读到被改写的值：
// sum = 1 + 2 + 100 = 103。
const POINTS = [{ x: 1 }, { x: 2 }, { x: 3 }];
function poison(i) {
  if (i === 1) {
    POINTS[2].x = 100;
  }
}
let sum = 0;
for (let i = 0; i < POINTS.length; i++) {
  sum = sum + POINTS[i].x;
  poison(i);
}
console.log(sum);
