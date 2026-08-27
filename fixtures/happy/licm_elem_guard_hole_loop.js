// Elem-guard 稀疏数组回归：字面量带洞（ArrayPushHole）→ 静态模板
// 收集即放弃；运行期 packed 校验同样会拒绝。sum = 1 + 3 = 4。
const POINTS = [{ x: 1 }, , { x: 3 }];
let sum = 0;
for (let i = 0; i < POINTS.length; i++) {
  const p = POINTS[i];
  if (p) {
    sum = sum + p.x;
  }
}
console.log(sum);
