// Elem-guard 原型污染回归：循环前替换 POINTS[1] 的原型。自有属性 x
// 始终遮蔽原型（守卫只保证自有模板键直读，原型键不参与快路径），
// 无论守卫命中与否 sum = 1 + 2 + 3 = 6；原型链上的 z 照常可读。
const POINTS = [{ x: 1 }, { x: 2 }, { x: 3 }];
Object.setPrototypeOf(POINTS[1], { x: 999, z: 7 });
let sum = 0;
for (let i = 0; i < POINTS.length; i++) {
  sum = sum + POINTS[i].x;
}
console.log(sum);
console.log(POINTS[1].z);
