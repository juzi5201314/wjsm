// 未捕获的静态字段初始化器异常必须在类定义期传播并终止执行
// （DefineField 的 ? Call）：后续静态元素与类后代码不得执行。
class Boom {
  static x = (() => {
    throw new Error("static-init-boom");
  })();
  static unreached = console.log("unreachable static");
}
console.log("unreachable", Boom);
