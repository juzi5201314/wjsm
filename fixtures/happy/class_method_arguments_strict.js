// 类体代码恒为严格模式（ClassDefinitionEvaluation）：async / generator /
// async generator 方法（公有与私有）的 arguments 均为 unmapped，写
// arguments 索引不改形参绑定。
class C {
  sync(a) { arguments[0] = 2; return a; }
  async as(a) { arguments[0] = 2; return a; }
  *gen(a) { arguments[0] = 2; yield a; }
  async *agen(a) { arguments[0] = 2; yield a; }
  async #p(a) { arguments[0] = 2; return a; }
  callP() { return this.#p(1); }
  *#g(a) { arguments[0] = 2; yield a; }
  callG() { return this.#g(1).next().value; }
  static async #s(a) { arguments[0] = 2; return a; }
  static callS() { return C.#s(1); }
}
const c = new C();
console.log("sync", c.sync(1));
console.log("gen", c.gen(1).next().value);
console.log("private-gen", c.callG());
c.as(1)
  .then((v) => {
    console.log("async", v);
    return c.agen(1).next();
  })
  .then((r) => {
    console.log("agen", r.value);
    return c.callP();
  })
  .then((v) => {
    console.log("private-async", v);
    return C.callS();
  })
  .then((v) => {
    console.log("private-static-async", v);
  });
