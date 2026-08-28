// yield* 委托的 GetIterator（§27.5.3.7 步骤 1，经 §7.4.3）必抛矩阵：nullish /
// 无 @@iterator / 方法非可调用 / 迭代器方法返回非对象都必须抛 TypeError，
// 而非把异常哨兵当普通值静默完成委托。sync 形态与 async 形态（@@asyncIterator
// 键）的文案与 Node v22 逐字节一致；唯一例外见文末 async 非可迭代块的注释。

function probe(label, generator) {
  try {
    generator().next();
    console.log(label, "no-throw");
  } catch (error) {
    console.log(label, error instanceof TypeError, error.message);
  }
}

probe("sync-null", function* () {
  yield* null;
});
probe("sync-undefined", function* () {
  yield* undefined;
});
probe("sync-object", function* () {
  yield* {};
});
probe("sync-number", function* () {
  yield* 1;
});
probe("sync-boolean", function* () {
  yield* true;
});
probe("sync-noncallable", function* () {
  yield* { [Symbol.iterator]: 1 };
});
probe("sync-nonobject", function* () {
  yield* {
    [Symbol.iterator]() {
      return 1;
    },
  };
});

// 生成器体内 try/catch 可捕获该 TypeError，捕获后还能继续 yield。
function* recovering() {
  try {
    yield* null;
  } catch (error) {
    yield error instanceof TypeError;
  }
}
const recovered = recovering();
console.log("sync-caught", recovered.next().value, recovered.next().done);

async function aprobe(label, generator) {
  try {
    await generator().next();
    console.log(label, "no-throw");
  } catch (error) {
    console.log(label, error instanceof TypeError, error.message);
  }
}

(async () => {
  await aprobe("async-null", async function* () {
    yield* null;
  });
  await aprobe("async-undefined", async function* () {
    yield* undefined;
  });
  await aprobe("async-noncallable", async function* () {
    yield* { [Symbol.asyncIterator]: 1 };
  });
  await aprobe("async-nonobject", async function* () {
    yield* {
      [Symbol.asyncIterator]() {
        return 1;
      },
    };
  });
  await aprobe("async-sync-nonobject", async function* () {
    yield* {
      [Symbol.iterator]() {
        return 1;
      },
    };
  });

  // GetIterator(value, async) 两个迭代器方法皆缺（§7.4.3 步骤 1.b.ii）：wjsm
  // 按 V8 kNotAsyncIterable 回退形态渲染（「object is not async iterable」）；
  // V8 的 async yield* desugar 直接调用 undefined 方法，Node 实际输出
  // 「undefined is not a function」。TypeError 类型与抛出时机一致，仅 callsite
  // 文案形态不同。
  await aprobe("async-object", async function* () {
    yield* {};
  });
  await aprobe("async-number", async function* () {
    yield* 1;
  });

  // async 生成器体内 try/catch 捕获后可继续 yield。
  async function* arecovering() {
    try {
      yield* undefined;
    } catch (error) {
      yield error instanceof TypeError;
    }
  }
  const arecovered = arecovering();
  console.log(
    "async-caught",
    (await arecovered.next()).value,
    (await arecovered.next()).done,
  );
})();
