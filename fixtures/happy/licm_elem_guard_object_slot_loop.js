// Elem-guard 对象槽回归：元素值槽存对象，二级读取 CELLS[i].v.n 的
// receiver 不是守卫读取结果 → 白名单静态拒绝；即使静态放行，运行期
// 值槽原始性校验也会拒绝。sum = 1 + 2 = 3。
const CELLS = [{ v: { n: 1 } }, { v: { n: 2 } }];
let sum = 0;
for (let i = 0; i < CELLS.length; i++) {
  sum = sum + CELLS[i].v.n;
}
console.log(sum);
