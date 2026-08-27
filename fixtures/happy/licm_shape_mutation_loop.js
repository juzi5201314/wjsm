// LICM 语义回归：循环体内写 P.x，读取不得外提。
// 每迭代必须读到当前值：sum = 3 + 4 + 5 + 6 + 7 = 25。
// 若 P.x 被错误外提，sum 恒为 3 * 5 = 15。
const P = { x: 3, y: 4 };
let sum = 0;
for (let i = 0; i < 5; i++) {
  sum = sum + P.x;
  P.x = P.x + 1;
}
console.log(sum);
console.log(P.x);
console.log(P);
