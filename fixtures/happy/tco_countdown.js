// 自递归尾调用降为循环后，结果与普通递归一致。
function countdown(n, acc) {
  if (n === 0) return acc;
  return countdown(n - 1, acc + n);
}
console.log(countdown(10, 0));
console.log(countdown(0, 7));
console.log(countdown(1, 0));
