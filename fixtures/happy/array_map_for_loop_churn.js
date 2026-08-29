// for 循环多次调用捕获 ARR 的 map（array_inline + 热循环头）。
const ARR = Array.from({ length: 400 }, (_, i) => i);
function work() {
  return ARR.map((x) => x % 3).length;
}
let sum = 0;
for (let i = 0; i < 6; i++) {
  sum += work();
}
console.log(sum);
