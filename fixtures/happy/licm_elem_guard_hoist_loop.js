// Elem-guard 外提正向用例：POINTS 是单赋值的统一模板对象字面量数组，
// 循环体只有守卫白名单指令。ElemShapeGuard 在 pre-header 一次性校验
// packed + 统一 shape + 值槽原始性，循环体内 POINTS[i].x 退化为
// 基址+偏移直读。sum = 1 + 3 + 5 = 9。
const POINTS = [
  { x: 1, y: 2 },
  { x: 3, y: 4 },
  { x: 5, y: 6 },
];
let sum = 0;
for (let i = 0; i < POINTS.length; i++) {
  sum += POINTS[i].x;
}
console.log(sum);
console.log(POINTS.length);
