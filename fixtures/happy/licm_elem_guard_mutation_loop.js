// Elem-guard 语义回归：循环体内 POINTS[i].z = i 扩展元素 shape，
// SetProp 不在白名单 → 静态放弃守卫，走通用路径。
// sum = (1+0) + (3+1) + (5+2) = 12。
const POINTS = [
  { x: 1, y: 2 },
  { x: 3, y: 4 },
  { x: 5, y: 6 },
];
let sum = 0;
for (let i = 0; i < POINTS.length; i++) {
  POINTS[i].z = i;
  sum = sum + POINTS[i].x + POINTS[i].z;
}
console.log(sum);
console.log(POINTS[2].z);
