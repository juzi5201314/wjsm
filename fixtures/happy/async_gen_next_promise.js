// #143: gen.next(value) 传入的 promise 应作为 yield 表达式结果直接传入，不被 await
async function* gen() {
  const x = yield;
  console.log("typeof", typeof x);
  console.log("not-awaited", x !== 42);
}
const g = gen();
g.next();
g.next(Promise.resolve(42));
