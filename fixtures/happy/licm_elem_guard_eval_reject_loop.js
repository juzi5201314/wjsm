// Elem-guard direct-eval 回归：eval 桥接会对每个可见绑定发出
// eval_get_binding + StoreVar 写回，bonus 因此获得不可证明原始的
// 写站点、POINTS 失去单赋值资格 → 静态放弃守卫，走通用路径，
// 每迭代 valueOf 正常参与 ToPrimitive。
// sum = (10+1) + (10+2) + (10+3) = 36。
const POINTS = [{ x: 1 }, { x: 2 }, { x: 3 }];
let bonus = 0;
eval("bonus = { valueOf: function () { return 10; } }");
let sum = 0;
for (let i = 0; i < POINTS.length; i++) {
  sum = sum + bonus + POINTS[i].x;
}
console.log(sum);
