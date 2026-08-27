// Elem-guard 运行期拒绝：循环前把 POINTS[1] 换成键序不同的对象。
// 绑定仍是单赋值（SetElem 不计入 StoreVar），静态计划守卫；运行期
// 元素 shape 与烘焙模板不符 → pre-header 守卫失败，整循环通用路径。
// sum = 1 + 20 + 3 = 24。
const POINTS = [{ x: 1 }, { x: 2 }, { x: 3 }];
POINTS[1] = { x: 20, extra: true };
let sum = 0;
for (let i = 0; i < POINTS.length; i++) {
  sum = sum + POINTS[i].x;
}
console.log(sum);
