// generator 函数族的 arguments 对象：wrapper 物化后经续体槽传入 body。
function* genDecl() {
  yield arguments.length;
  yield arguments[0] + arguments[1];
}
const itDecl = genDecl(40, 2);
console.log(itDecl.next().value);
console.log(itDecl.next().value);

// 跨 yield 的 arguments 身份保持一致（每次调用恰好一个 arguments 对象）。
function* genIdentity() {
  const saved = arguments;
  yield 1;
  yield saved === arguments;
}
const itId = genIdentity(9);
itId.next();
console.log(itId.next().value);

// 表达式与方法形式。
const genExpr = function* () {
  yield arguments.length;
};
console.log(genExpr(1, 2, 3).next().value);

const obj = {
  *m() {
    yield arguments.length;
  },
};
console.log(obj.m(7).next().value);

class C {
  *m() {
    yield arguments.length;
  }
  static *s() {
    yield arguments[0];
  }
}
console.log(new C().m(1, 2, 3, 4).next().value);
console.log(C.s(11).next().value);

// 尾随逗号（test262 trailing-comma 变体）。
function* genTrailing() {
  yield arguments.length;
}
console.log(genTrailing(42, "TC39",).next().value);
