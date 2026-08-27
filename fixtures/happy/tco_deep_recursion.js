// 尾调用深度远超 native 栈上限；TCO 后不应触发 RangeError。
// 该 fixture 的期望输出不能用 Node.js oracle 生成：V8 未实现 proper tail call，
// 同样的代码在 Node 上抛 RangeError。调用栈深度上限是实现限制而非规范要求，
// 消除尾调用栈帧不改变任何规范可观察语义。
function count(n, acc) {
  if (n === 0) return acc;
  return count(n - 1, acc + 1);
}
console.log(count(200000, 0));
