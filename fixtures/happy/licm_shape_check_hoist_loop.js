// LICM & Shape 检查外提：P 是稳定 record（自有数据属性、引用只逃逸到
// console.log），循环体内的常量键读取 P.x 连同其 Inline Cache 检查整体
// 外提到 pre-header，循环内退化为寄存器复用。5 次迭代 sum = 5 * 3 = 15。
const P = { x: 3, y: 4 };
let sum = 0;
for (let i = 0; i < 5; i++) {
  sum = sum + P.x;
}
console.log(sum);
console.log(P);
