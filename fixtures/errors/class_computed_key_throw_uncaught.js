// 未捕获的类计算键异常必须在类定义期传播并终止执行：
// 键表达式先于字段值/后续成员求值抛出，类绑定不得完成初始化。
function boom() {
  throw new Error("class key boom");
}
class C {
  [boom()] = 1;
  static unreached = console.log("unreachable static");
}
console.log("unreachable", C);
