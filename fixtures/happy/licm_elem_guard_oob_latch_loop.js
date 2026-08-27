// Elem-guard 单向闩锁：循环上界 5 超出数组长度 3。前 3 次迭代守卫
// 快路径直读；i=3 时 GetElemGuarded 越界 miss → 先熄灭守卫再走宿主
// GetElem，之后所有访问退回通用路径，输出必须与完全未优化一致
// （当前引擎 undefined.x 通用路径返回 undefined → sum 变 NaN）。
const POINTS = [{ x: 1 }, { x: 2 }, { x: 3 }];
let sum = 0;
for (let i = 0; i < 5; i++) {
  sum = sum + POINTS[i].x;
  console.log(sum);
}
