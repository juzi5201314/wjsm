// LICM 语义回归：receiver 绑定在循环体内被改写，读取不得外提。
// 迭代读到 P.x, P.x, Q.x, Q.x：sum = 1 + 1 + 2 + 2 = 6。
const P = { x: 1 };
const Q = { x: 2 };
let pick = P;
let sum = 0;
for (let i = 0; i < 4; i++) {
  sum = sum + pick.x;
  if (i == 1) {
    pick = Q;
  }
}
console.log(sum);
console.log(P);
console.log(Q);
