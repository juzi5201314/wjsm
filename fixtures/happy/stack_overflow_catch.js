// 非尾位置递归（`+ 1` 在调用之后求值）：不会被 tail_self_loop 改写成循环，
// 因此仍然线性消耗调用栈并抛出 RangeError。
function recurse(n) {
  return recurse(n + 1) + 1;
}
try {
  recurse(0);
} catch (error) {
  console.log(error instanceof RangeError);
  console.log(error.name);
  console.log(error.message);
  console.log(error.stack);
}
console.log("continued");
