// 类 / 对象字面量的 async 方法与 async generator 方法 / 表达式的路由：
// 必须返回真正的 Promise / AsyncGenerator，且 this、arguments、await 均正确。
class C {
  constructor() {
    this.x = 5;
  }
  async plain() {
    return this.x;
  }
  async withAwait(inc) {
    const v = await Promise.resolve(this.x);
    return v + inc;
  }
  async *gen() {
    yield this.x + arguments.length;
    await Promise.resolve();
    yield this.x + 1;
  }
  static async *staticGen() {
    yield arguments[0] * 2;
  }
}

const obj = {
  y: 7,
  async m() {
    return this.y + arguments.length;
  },
  async *g() {
    yield arguments.length;
  },
};

const asyncGenExpr = async function* () {
  yield arguments.length;
};

(async () => {
  const c = new C();
  console.log(typeof c.plain().then);
  console.log(await c.plain());
  console.log(await c.withAwait(10));
  const it = c.gen(1, 2);
  console.log((await it.next()).value);
  console.log((await it.next()).value);
  console.log((await C.staticGen(21).next()).value);
  console.log(await obj.m(1, 2, 3));
  console.log((await obj.g(9).next()).value);
  console.log((await asyncGenExpr(1, 2, 3, 4).next()).value);
})();
