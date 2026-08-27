// Elem-guard 运行期拒绝：循环前 defineProperty 把 POINTS[1].x 改成
// accessor，元素 shape 偏离烘焙模板 → pre-header 守卫失败，整个循环
// 走通用 IC 路径，getter 副作用恰好触发一次。
// sum = 1 + 10 + 3 = 14，n = 1。
let n = 0;
const POINTS = [{ x: 1 }, { x: 2 }, { x: 3 }];
Object.defineProperty(POINTS[1], "x", {
  get() {
    n = n + 1;
    return 10 * n;
  },
});
let sum = 0;
for (let i = 0; i < POINTS.length; i++) {
  sum = sum + POINTS[i].x;
}
console.log(sum);
console.log(n);
