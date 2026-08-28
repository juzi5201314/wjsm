// issue #400 回归：generator 函数族 arguments 经续体槽传参。
// 覆盖 test262 *-gen-*-args-trailing-comma-* 矩阵的全部同步 generator 形态：
// gen-func-decl / gen-func-expr / gen-meth / cls-decl-gen-meth(-static) /
// cls-expr-gen-meth(-static)，尾随逗号变体 multiple / single / spread / null / undefined。

function fmt(args) {
  let parts = [];
  for (let i = 0; i < args.length; i += 1) {
    parts.push(String(args[i]));
  }
  return parts.join("|") + " #" + args.length;
}

// gen-func-decl × multiple
function* genDecl() {
  yield fmt(arguments);
}
console.log(genDecl(42, "TC39",).next().value);

// gen-func-expr × single
const genExpr = function* () {
  yield fmt(arguments);
};
console.log(genExpr(1,).next().value);

// gen-meth（对象字面量方法）× spread
const obj = {
  *m() {
    yield fmt(arguments);
  },
};
console.log(obj.m(...[7, 8, 9],).next().value);

// cls-decl-gen-meth × null；cls-decl-gen-meth-static × undefined
class Decl {
  *m() {
    yield fmt(arguments);
  }
  static *s() {
    yield fmt(arguments);
  }
}
console.log(new Decl().m(null,).next().value);
console.log(Decl.s(undefined,).next().value);

// cls-expr-gen-meth × multiple；cls-expr-gen-meth-static × spread
const Expr = class {
  *m() {
    yield fmt(arguments);
  }
  static *s() {
    yield fmt(arguments);
  }
};
console.log(new Expr().m(42, "TC39",).next().value);
console.log(Expr.s(...[4, 2],).next().value);
