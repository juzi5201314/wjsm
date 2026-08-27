// LICM 语义回归：0 次进入的循环。被外提到 pre-header 的读取多执行一次
// 也必须不可观察（无副作用、不抛异常），sum 保持 0。
const P = { x: 3, y: 4 };
let sum = 0;
for (let i = 0; i < 0; i++) {
  sum = sum + P.x;
}
console.log(sum);
console.log(P);
