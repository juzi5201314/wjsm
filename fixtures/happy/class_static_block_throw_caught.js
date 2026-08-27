// static block 异常可被外层 try/catch 捕获（类定义期传播），
// 捕获后继续执行，与静态字段初始化器异常行为一致。
try {
  class C {
    static {
      throw new Error("block-x");
    }
  }
  console.log("unreachable after class");
} catch (e) {
  console.log("caught-block:", e.message);
}

// 类表达式的 static block 异常同样传播到调用方。
function makeClass() {
  return class {
    static {
      throw new Error("expr-x");
    }
  };
}
try {
  makeClass();
} catch (e) {
  console.log("caught-expr:", e.message);
}

// 静态元素按源顺序执行：先执行的静态字段可观察，
// 抛出后剩余静态元素不执行。
try {
  class Ordered {
    static first = console.log("field-first");
    static {
      throw new Error("ordered-x");
    }
    static last = console.log("unreachable field");
  }
} catch (e) {
  console.log("caught-ordered:", e.message);
}
console.log("done");
