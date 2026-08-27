// Elem-guard 零迭代回归：守卫在 pre-header 无条件求值一次，必须无
// 副作用；循环体一次不执行，sum 保持 0。
const POINTS = [{ x: 1 }, { x: 2 }];
let limit = 0;
let sum = 0;
for (let i = 0; i < limit; i++) {
  sum = sum + POINTS[i].x;
}
console.log(sum);
