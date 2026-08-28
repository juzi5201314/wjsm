// §7.4.6 IteratorClose 步骤 5：completion 为 throw 时原始异常胜出（return
// 查找抛出 / 非 callable / 调用抛出 / 返回非对象全部吞咽）；§7.4.7/§7.4.8：
// next/done/value 自身抛出置 [[Done]] 为 true，其后不再调用 return()，
// 耗尽后（done: true）的正常完成与 abrupt completion 同样不触发 close。
function counting(next) {
  let ret = 0;
  const iterable = {
    [Symbol.iterator]: () => ({
      next,
      return() {
        ret += 1;
        return {};
      },
    }),
  };
  return { iterable, calls: () => ret };
}

// next() 抛出 → [[Done]] true，不 close
{
  const { iterable, calls } = counting(() => {
    throw new Error("next-boom");
  });
  try {
    const [a] = iterable;
  } catch (e) {
    console.log("next throw:", e.message);
  }
  console.log("next throw return calls:", calls());
}

// value getter 抛出 → [[Done]] true，不 close
{
  const { iterable, calls } = counting(() => ({
    done: false,
    get value() {
      throw new Error("value-boom");
    },
  }));
  try {
    const [a] = iterable;
  } catch (e) {
    console.log("value throw:", e.message);
  }
  console.log("value throw return calls:", calls());
}

// done getter 抛出 → [[Done]] true，不 close
{
  const { iterable, calls } = counting(() => ({
    get done() {
      throw new Error("done-boom");
    },
  }));
  try {
    const [a] = iterable;
  } catch (e) {
    console.log("done throw:", e.message);
  }
  console.log("done throw return calls:", calls());
}

// 迭代器耗尽后默认值抛出 → [[Done]] true，不 close
{
  const { iterable, calls } = counting(() => ({ done: true }));
  try {
    const [a = (() => {
      throw new Error("default-after-done");
    })()] = iterable;
  } catch (e) {
    console.log("after done:", e.message);
  }
  console.log("after done return calls:", calls());
}

// 耗尽后的正常完成 → 不 close；done 结果的 value getter 不可观察
{
  let reads = 0;
  const { iterable, calls } = counting(() => ({
    done: true,
    get value() {
      reads += 1;
      return 42;
    },
  }));
  const [a] = iterable;
  console.log("exhausted normal a:", a, "return calls:", calls(), "value reads:", reads);
}

// 未耗尽正常完成 → close 一次
{
  const { iterable, calls } = counting(() => ({ done: false, value: 7 }));
  const [a] = iterable;
  console.log("normal a:", a, "return calls:", calls());
}

// rest 收集耗尽后正常完成 → 不 close
{
  let n = 0;
  const { iterable, calls } = counting(() =>
    n < 3 ? { done: false, value: ++n } : { done: true },
  );
  const [...r] = iterable;
  console.log("rest r:", r.join("/"), "return calls:", calls());
}

// throw completion：setter 抛出 + return() 抛出 → 原始异常胜出
{
  const iterable = {
    [Symbol.iterator]: () => ({
      next: () => ({ done: false, value: 1 }),
      return() {
        throw new Error("close-boom");
      },
    }),
  };
  const o = {
    set x(v) {
      throw new Error("orig-wins");
    },
  };
  try {
    [o.x] = iterable;
  } catch (e) {
    console.log("close throw swallowed:", e.message);
  }
}

// throw completion：return() 返回非对象 → 吞咽，原始异常胜出
{
  const iterable = {
    [Symbol.iterator]: () => ({
      next: () => ({ done: false, value: 1 }),
      return: () => 42,
    }),
  };
  const o = {
    set x(v) {
      throw new Error("orig-wins-2");
    },
  };
  try {
    [o.x] = iterable;
  } catch (e) {
    console.log("non-object swallowed:", e.message);
  }
}

// throw completion：return 属性为抛错 getter → 吞咽，原始异常胜出
{
  const iterable = {
    [Symbol.iterator]: () => ({
      next: () => ({ done: false, value: 1 }),
      get return() {
        throw new Error("return-getter-boom");
      },
    }),
  };
  const o = {
    set x(v) {
      throw new Error("orig-wins-3");
    },
  };
  try {
    [o.x] = iterable;
  } catch (e) {
    console.log("getter swallowed:", e.message);
  }
}

// 正常完成关闭：return() 抛出 → 传播（§7.4.6 步骤 6）
{
  const iterable = {
    [Symbol.iterator]: () => ({
      next: () => ({ done: false, value: 1 }),
      return() {
        throw new Error("normal-close-throw");
      },
    }),
  };
  try {
    const [a] = iterable;
  } catch (e) {
    console.log("normal close throw:", e.message);
  }
}

// 正常完成关闭：return() 返回非对象 → TypeError（§7.4.6 步骤 7）
{
  const iterable = {
    [Symbol.iterator]: () => ({
      next: () => ({ done: false, value: 1 }),
      return: () => 42,
    }),
  };
  try {
    const [a] = iterable;
  } catch (e) {
    console.log("normal close non-object:", e.name);
  }
}
