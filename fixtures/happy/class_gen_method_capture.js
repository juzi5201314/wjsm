// 类 generator 方法对外层捕获变量的写回：
// wrapper 的闭包 env 必须是共享 env 本身（不能包持有 home 的 method env），
// 否则 body 内深度 0 的捕获写会遮蔽共享 env，外层读不到写回。
// 矩阵：var/let × 实例/static × 同步 generator/async generator × 普通/复合赋值。
var vCount = 0;
let lCount = 0;

class Counter {
  *bump() {
    vCount = vCount + 1;
    yield vCount;
    vCount += 10;
  }

  static *staticBump() {
    lCount = lCount + 1;
    yield lCount;
  }
}

const it = new Counter().bump();
console.log("instance-var", it.next().value, vCount);
it.next();
console.log("instance-var-compound", vCount);

Counter.staticBump().next();
console.log("static-let", lCount);

// 同一共享 env 在类求值后仍与外层普通读写保持一致。
vCount = 100;
const it2 = new Counter().bump();
console.log("reuse", it2.next().value, vCount);

class AsyncCounter {
  async *tick() {
    lCount += 100;
    yield lCount;
  }
}

new AsyncCounter().tick().next().then((r) => {
  console.log("async-gen", r.value, lCount);
});
console.log("sync-tail", lCount);
