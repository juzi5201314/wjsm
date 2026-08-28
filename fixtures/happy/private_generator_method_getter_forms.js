// issue #399 回归：私有 generator 方法（*#method）经 getter 取出为函数值后调用 .next()。
// 崩溃形态是 PrivateGet 取出的 generator wrapper 作为独立函数值被调用；
// 覆盖 test262 cls-{decl,expr}-private-gen-meth(-static)-args-trailing-comma-* 矩阵：
// 实例/static × 声明/表达式 × 尾随逗号变体 multiple / single / spread / null / undefined。

function fmt(args) {
  let parts = [];
  for (let i = 0; i < args.length; i += 1) {
    parts.push(String(args[i]));
  }
  return parts.join("|") + " #" + args.length;
}

// cls-decl-private-gen-meth × multiple；cls-decl-private-gen-meth-static × single
class Decl {
  *#m() {
    yield fmt(arguments);
  }
  get method() {
    return this.#m;
  }
  static *#s() {
    yield fmt(arguments);
  }
  static get method() {
    return this.#s;
  }
}
console.log(new Decl().method(42, "TC39",).next().value);
console.log(Decl.method(1,).next().value);

// cls-expr-private-gen-meth × spread；cls-expr-private-gen-meth-static × null
const Expr = class {
  *#m() {
    yield fmt(arguments);
  }
  get method() {
    return this.#m;
  }
  static *#s() {
    yield fmt(arguments);
  }
  static get method() {
    return this.#s;
  }
};
console.log(new Expr().method(...[7, 8, 9],).next().value);
console.log(Expr.method(null,).next().value);

// undefined 变体 + issue 原始复现：return 值经 .next().value 读取
class Ret {
  *#m() {
    return 42;
  }
  get method() {
    return this.#m;
  }
}
console.log(new Ret().method(undefined,).next().value);

// 取出后的 wrapper 是普通 generator 函数值：可脱离实例存放再调用，
// this 由调用形式决定（getter 已完成 brand 检查与取值）。
const detached = new Decl().method;
console.log(detached("a", "b",).next().value);
