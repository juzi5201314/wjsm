// issue #400 回归：async generator 函数族 arguments 传参与路由。
// 此前 method / named-func-expr / class 表达式形态直接 InternalInvariant 崩溃。
// 覆盖 async-gen-func-decl / async-gen-named-func-expr / async-gen-meth /
// cls-decl-async-gen-meth(-static) / cls-expr-async-gen-meth(-static)，
// 尾随逗号变体 multiple / single / spread / null / undefined。

function fmt(args) {
  let parts = [];
  for (let i = 0; i < args.length; i += 1) {
    parts.push(String(args[i]));
  }
  return parts.join("|") + " #" + args.length;
}

// async-gen-func-decl
async function* agDecl() {
  yield fmt(arguments);
}

// async-gen-named-func-expr
const agNamed = async function* named() {
  yield fmt(arguments);
};

// async-gen-meth（对象字面量方法）
const obj = {
  async *m() {
    yield fmt(arguments);
  },
};

// cls-decl-async-gen-meth（含 static）
class Decl {
  async *m() {
    yield fmt(arguments);
  }
  static async *s() {
    yield fmt(arguments);
  }
}

// cls-expr-async-gen-meth（含 static）
const Expr = class {
  async *m() {
    yield fmt(arguments);
  }
  static async *s() {
    yield fmt(arguments);
  }
};

// 跨 yield/await 的 arguments 身份保持一致（每次调用恰好一个 arguments 对象）。
async function* agIdentity() {
  const saved = arguments;
  yield 1;
  await Promise.resolve();
  yield saved === arguments;
}

(async () => {
  console.log((await agDecl(42, "TC39",).next()).value);
  console.log((await agNamed(1,).next()).value);
  console.log((await obj.m(...[7, 8, 9],).next()).value);
  console.log((await new Decl().m(null,).next()).value);
  console.log((await Decl.s(undefined,).next()).value);
  console.log((await new Expr().m(42, "TC39",).next()).value);
  console.log((await Expr.s(...[4, 2],).next()).value);
  const it = agIdentity(9);
  await it.next();
  console.log((await it.next()).value);
})();
