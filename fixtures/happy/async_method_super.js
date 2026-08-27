// async 方法 / async generator 方法内的 super 绑定：
// 类方法经静态 [[HomeObject]] 元数据、对象字面量方法经续体 `home` 属性接线，
// super 属性读 / 调用 / 赋值在 await 前后都必须正确。
class A {
  constructor() {
    this.tag = "A";
  }
  m() {
    return 1;
  }
  get p() {
    return 5;
  }
  who() {
    return this.tag;
  }
  static s() {
    return 7;
  }
}

let counter = 0;

class B extends A {
  constructor() {
    super();
    this.tag = "B";
  }
  async callBeforeAwait() {
    return super.m();
  }
  async callAfterAwait() {
    await Promise.resolve();
    return super.m() + 10;
  }
  async readAccessor() {
    return super.p;
  }
  async thisReceiver() {
    // super 调用的 receiver 必须是当前 this（读到派生实例的字段）。
    return super.who();
  }
  async assignThrough() {
    // super.x = v 按 ReflectSet 定义在 this 上。
    super.y = 42;
    return this.y;
  }
  async arrowAfterResume() {
    const f = () => super.m();
    await Promise.resolve();
    return f() + 20;
  }
  async writeCaptured() {
    // 方法 env 外包一层 home 对象后，外层捕获变量的写入必须定位真正 owner。
    counter++;
  }
  static async staticSuper() {
    return super.s();
  }
  async *genSuper() {
    yield super.m();
    await Promise.resolve();
    yield super.m() + 100;
  }
}

const base = {
  m() {
    return 2;
  },
};

const derived = {
  __proto__: base,
  async m() {
    const before = super.m();
    await Promise.resolve();
    return before + super.m();
  },
  async *g() {
    yield super.m();
  },
};

(async () => {
  const b = new B();
  console.log(await b.callBeforeAwait());
  console.log(await b.callAfterAwait());
  console.log(await b.readAccessor());
  console.log(await b.thisReceiver());
  console.log(await b.assignThrough());
  console.log(await b.arrowAfterResume());
  await b.writeCaptured();
  console.log(counter);
  console.log(await B.staticSuper());
  const it = b.genSuper();
  console.log((await it.next()).value);
  console.log((await it.next()).value);
  console.log(await derived.m());
  console.log((await derived.g().next()).value);
})();
