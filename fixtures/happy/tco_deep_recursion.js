// tail_self_loop 把这个自递归尾调用改写成回边循环，20 万层不再消耗调用栈。
// 注意：Node.js（V8 未实现 proper tail call）在这里会抛 RangeError，
// 因此本 fixture 的 .expected 是 wjsm 专有行为，不能用 oracle 自动更新。
function count(n, acc) {
  if (n === 0) return acc;
  return count(n - 1, acc + 1);
}
console.log(count(200000, 0));
