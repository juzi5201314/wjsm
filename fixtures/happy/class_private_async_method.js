// 私有普通 async 方法：经 async 函数表达式路径的 body + wrapper 双函数结构降级，
// wrapper 函数值由 PrivateSet 绑定到实例/构造器；await 语义与公有 async 方法一致。
// 矩阵：实例/static × await 恢复 this/私有字段 × 捕获写回 × 参数/默认值/rest/arguments
// × super × 链式私有调用 × 异常传播/try-catch × brand 检查。
let outer = 0;

class Base {
  greet() {
    return "base";
  }
}

class Task extends Base {
  #step = 10;

  async #add(extra) {
    outer += 1;
    const base = await Promise.resolve(this.#step);
    return base + extra;
  }

  run(extra) {
    return this.#add(extra);
  }

  async #plain() {
    return "plain";
  }

  plainIsPromise() {
    const result = this.#plain();
    return typeof result.then === "function";
  }

  async #withParams(a, b = 2, ...rest) {
    await Promise.resolve();
    return [a, b, rest.length, arguments.length].join(",");
  }

  params() {
    return this.#withParams(1, undefined, 9, 8);
  }

  async #viaSuper() {
    const before = super.greet();
    await Promise.resolve();
    return before + "+" + super.greet();
  }

  callSuper() {
    return this.#viaSuper();
  }

  async #mutate() {
    await Promise.resolve();
    this.#step += 1;
    return this.#step;
  }

  async #chain() {
    const first = await this.#mutate();
    const second = await this.#mutate();
    return first + ":" + second;
  }

  chain() {
    return this.#chain();
  }

  async #boom() {
    await Promise.resolve();
    throw new Error("kaboom");
  }

  async #recover() {
    try {
      await this.#boom();
      return "no-throw";
    } catch (error) {
      return "caught:" + error.message;
    }
  }

  recover() {
    return this.#recover();
  }

  reject() {
    return this.#boom();
  }

  static async #stamp(kind) {
    const suffix = await Promise.resolve("ok");
    return kind + "-" + suffix;
  }

  static stamp(kind) {
    return Task.#stamp(kind);
  }

  static tryCall(target) {
    try {
      target.#add(0);
      return "callable";
    } catch (error) {
      return error instanceof TypeError ? "TypeError" : "other";
    }
  }
}

const task = new Task();
console.log("plain-is-promise", task.plainIsPromise());
console.log("brand", Task.tryCall(task), Task.tryCall({}));
task
  .run(5)
  .then((value) => {
    console.log("instance", value, outer);
    return Task.stamp("static");
  })
  .then((value) => {
    console.log("static", value);
    return task.params();
  })
  .then((value) => {
    console.log("params", value);
    return task.callSuper();
  })
  .then((value) => {
    console.log("super", value);
    return task.chain();
  })
  .then((value) => {
    console.log("chain", value);
    return task.recover();
  })
  .then((value) => {
    console.log("recover", value);
    return task.reject().catch((error) => "rejected:" + error.message);
  })
  .then((value) => {
    console.log("reject", value);
  });
