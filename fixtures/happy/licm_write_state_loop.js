// LICM 语义回归：写持久状态的纯化函数不得被提升出循环
// （function work(){ counter++; } 通过闭包变量路径写 $0.counter）。
// 若被提升，counter 只加一次 → 输出 1 而非 100。
let counter = 0;
function work() {
  counter++;
}
let i = 0;
while (i < 100) {
  work();
  i++;
}
console.log(counter);
