// 未捕获的 static block 异常必须在类定义期传播并终止执行
// （ClassStaticBlockDefinition 的 ? Call）：先执行的静态块可观察，
// 后续静态块、静态字段与类后代码都不得执行。
class Boom {
  static {
    console.log("first-block");
  }
  static {
    throw new Error("static-block-boom");
  }
  static {
    console.log("unreachable block");
  }
  static unreached = console.log("unreachable field");
}
console.log("unreachable", Boom);
