// 模块内同伴函数调用带自捕获的递归函数（fib30 / work→fib 回归）。
// 默认按 module 解析：不可把 GetProp($env,"$0.fib") 内联成「无 lex_env +
// caller $env 冒充」，否则自递归读到非函数。
function fib(n) {
  if (n < 2) return n;
  return fib(n - 1) + fib(n - 2);
}
function work() {
  return fib(10);
}
console.log(work());
