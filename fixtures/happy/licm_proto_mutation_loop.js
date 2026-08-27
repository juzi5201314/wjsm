// LICM 语义回归：循环体内改写原型链。自有数据属性 x 的读取不受原型影响
// （sum = 3 * 1 = 3），新原型的 z 在循环后可见（9）。
const P = { x: 1 };
let sum = 0;
for (let i = 0; i < 3; i++) {
  sum = sum + P.x;
  Object.setPrototypeOf(P, { z: 9 });
}
console.log(sum);
console.log(P.z);
