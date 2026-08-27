// 私有 generator / async generator 方法：经声明路径的 body + wrapper 双函数结构降级，
// wrapper 函数值由 PrivateSet 绑定到实例/构造器；yield 语义与公有 generator 方法一致。
// 矩阵：实例/static × 同步/async generator × 捕获写回/this 字段/参数/arguments/yield* 委托/brand 检查。
let outer = 0;

class Seq {
  #base = 10;

  *#gen(step) {
    outer += 1;
    yield this.#base + step;
    yield this.#base + step * 2;
  }

  collect(step) {
    return [...this.#gen(step)];
  }

  static *#sgen() {
    yield "s1";
    yield "s2";
  }

  static collectStatic() {
    return [...Seq.#sgen()];
  }

  *#inner() {
    yield 1;
    yield 2;
  }

  *#outer() {
    yield 0;
    yield* this.#inner();
    yield 3;
  }

  delegated() {
    return [...this.#outer()];
  }

  *#viaArguments() {
    yield arguments.length;
    yield arguments[1];
  }

  argInfo() {
    return [...this.#viaArguments("a", "b", "c")];
  }

  async *#agen() {
    yield 1;
    yield await Promise.resolve(2);
  }

  async sum() {
    const it = this.#agen();
    let total = 0;
    let r = await it.next();
    while (!r.done) {
      total += r.value;
      r = await it.next();
    }
    return total;
  }

  static async *#sagen() {
    yield "x";
    yield "y";
  }

  static async joinStatic() {
    const it = Seq.#sagen();
    const parts = [];
    let r = await it.next();
    while (!r.done) {
      parts.push(r.value);
      r = await it.next();
    }
    return parts.join("+");
  }

  static tryCall(target) {
    try {
      target.#gen(0);
      return "callable";
    } catch (error) {
      return error instanceof TypeError ? "TypeError" : "other";
    }
  }
}

const seq = new Seq();
console.log("instance", seq.collect(1).join(","), outer);
console.log("instance-again", seq.collect(2).join(","), outer);
console.log("static", Seq.collectStatic().join(","));
console.log("delegate", seq.delegated().join(","));
console.log("arguments", seq.argInfo().join(","));
console.log("brand", Seq.tryCall(seq), Seq.tryCall({}));
seq.sum().then((total) => console.log("async-gen", total));
Seq.joinStatic().then((joined) => console.log("static-async-gen", joined));
