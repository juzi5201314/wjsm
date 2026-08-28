// §8.6.2 IteratorBindingInitialization / §13.15.5.2 步骤 3：数组解构元素
// 初始化的 abrupt completion（默认值求值、成员 setter、嵌套 pattern 抛出）
// 在迭代器未耗尽（[[Done]] 为 false）时执行 IteratorClose；step 自身抛出或
// 耗尽后的 abrupt 置 [[Done]] 为 true，不再调用 return()。
function makeIter(value) {
  let ret = 0;
  const iterable = {
    [Symbol.iterator]: () => ({
      next: () => ({ done: false, value }),
      return() {
        ret += 1;
        return {};
      },
    }),
  };
  return { iterable, calls: () => ret };
}

// 成员 setter 抛出 → close
{
  const { iterable, calls } = makeIter(1);
  const o = {
    set x(v) {
      throw new Error("setter-boom");
    },
  };
  try {
    [o.x] = iterable;
  } catch (e) {
    console.log("setter:", e.message);
  }
  console.log("setter return calls:", calls());
}

// 声明型解构默认值抛出 → close
{
  const { iterable, calls } = makeIter(undefined);
  try {
    const [a = (() => {
      throw new Error("default-boom");
    })()] = iterable;
  } catch (e) {
    console.log("default:", e.message);
  }
  console.log("default return calls:", calls());
}

// 赋值型解构默认值抛出 → close
{
  const { iterable, calls } = makeIter(undefined);
  let a;
  try {
    [a = (() => {
      throw new Error("assign-default-boom");
    })()] = iterable;
  } catch (e) {
    console.log("assign default:", e.message);
  }
  console.log("assign default return calls:", calls());
}

// 嵌套对象 pattern 的 getter 抛出 → close
{
  const { iterable, calls } = makeIter({
    get p() {
      throw new Error("nested-getter-boom");
    },
  });
  try {
    const [{ p }] = iterable;
  } catch (e) {
    console.log("nested:", e.message);
  }
  console.log("nested return calls:", calls());
}

// 函数参数解构默认值抛出 → close
{
  const { iterable, calls } = makeIter(undefined);
  function f([a = (() => {
    throw new Error("param-default-boom");
  })()]) {}
  try {
    f(iterable);
  } catch (e) {
    console.log("param:", e.message);
  }
  console.log("param return calls:", calls());
}

// elision 后第二元素 setter 抛出 → next 两次、close 一次
{
  let ret = 0;
  let n = 0;
  const iterable = {
    [Symbol.iterator]: () => ({
      next: () => ({ done: false, value: ++n }),
      return() {
        ret += 1;
        return {};
      },
    }),
  };
  const o = {
    set x(v) {
      throw new Error("after-hole-boom");
    },
  };
  try {
    [, o.x] = iterable;
  } catch (e) {
    console.log("elision:", e.message);
  }
  console.log("elision next calls:", n, "return calls:", ret);
}

// 嵌套数组 pattern：内层 setter 抛出 → 内外迭代器都 close（内层先）
{
  const log = [];
  const mk = (tag, value) => ({
    [Symbol.iterator]: () => ({
      next: () => ({ done: false, value }),
      return() {
        log.push(tag);
        return {};
      },
    }),
  });
  const inner = mk("inner-ret", 5);
  const outer = mk("outer-ret", inner);
  const o = {
    set x(v) {
      throw new Error("deep-boom");
    },
  };
  try {
    [[o.x]] = outer;
  } catch (e) {
    log.push("catch:" + e.message);
  }
  console.log("nested arrays:", log.join(","));
}

// try/catch：close 先于 catch 体执行
{
  const log = [];
  const iterable = {
    [Symbol.iterator]: () => ({
      next: () => ({ done: false, value: 1 }),
      return() {
        log.push("return()");
        return {};
      },
    }),
  };
  const o = {
    set x(v) {
      throw new Error("caught-boom");
    },
  };
  try {
    [o.x] = iterable;
  } catch (e) {
    log.push("catch");
  }
  console.log("close before catch:", log.join(","));
}

// generator 源：close 触发 return() → finally 执行
{
  const log = [];
  function* gen() {
    try {
      yield 1;
      yield 2;
    } finally {
      log.push("gen-finally");
    }
  }
  const o = {
    set x(v) {
      log.push("set" + v);
      throw new Error("gen-boom");
    },
  };
  try {
    [o.x] = gen();
  } catch (e) {
    log.push("catch:" + e.message);
  }
  console.log("generator:", log.join(","));
}

// for-of 体内解构 setter 抛出 → 内迭代器先 close，再 close 外层 for-of 迭代器
{
  const log = [];
  const mk = (tag, value) => ({
    [Symbol.iterator]: () => ({
      next: () => ({ done: false, value }),
      return() {
        log.push(tag);
        return {};
      },
    }),
  });
  const innerIterable = mk("inner-ret", 3);
  const o = {
    set x(v) {
      throw new Error("in-forof-boom");
    },
  };
  try {
    for (const q of mk("outer-ret", 0)) {
      [o.x] = innerIterable;
    }
  } catch (e) {
    log.push("catch");
  }
  console.log("for-of nesting:", log.join(","));
}
