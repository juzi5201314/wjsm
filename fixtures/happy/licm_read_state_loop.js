// LICM 语义回归：读持久状态的函数不得被提升出循环。
// acc 每迭代必须读到 counter 的当前值（0+1+...+99 = 4950）。
// 若 work() 被提升，acc 恒为 0。
let counter = 0;
function work() {
  return counter;
}
let i = 0;
let acc = 0;
while (i < 100) {
  acc += work();
  counter++;
  i++;
}
console.log(acc);
