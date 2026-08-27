// 未捕获的调用实参异常必须终止执行并以运行时错误退出，
// 而不是把异常哨兵当作实参传入（曾打印 [object Object]）。
function f() {
  throw new Error("boom");
}
console.log(f());
console.log("unreachable");
