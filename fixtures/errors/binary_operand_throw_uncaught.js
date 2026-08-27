// 未捕获的二元运算操作数异常（ES §13.15.4 `? GetValue(rval)`）必须终止执行
// 并以运行时错误退出，不得被字符串拼接吞掉后打印 "[object Object]" 继续执行。
function fail() {
  throw new TypeError("operand boom");
}
console.log("x: " + fail());
console.log("unreachable");
