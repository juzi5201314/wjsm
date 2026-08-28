// async generator 的 yield* 委托（§27.5.3.7 的 async 形态）：逐值转发 async
// 迭代器与 sync 可迭代（AsyncFromSyncIterator 语义包裹），委托表达式的值为
// 最终 result.value（步骤 7.a.iii）；内部迭代器抛出经委托层传播到外层
// try/catch；return() 中断委托后外层立即完成。输出与 Node v22 逐字节一致。

async function* inner() {
  yield 1;
  yield 2;
  return "inner-done";
}

async function* outer() {
  const received = yield* inner();
  console.log("received", received);
  yield* [3, 4];
  yield* "ab";
  yield* new Set([5]);
  return "outer-done";
}

async function* throwing() {
  yield "t1";
  throw new RangeError("boom");
}

async function* catching() {
  try {
    yield* throwing();
  } catch (error) {
    console.log("caught", error.constructor.name, error.message);
    yield "recovered";
  }
}

(async () => {
  const iterator = outer();
  let step;
  while (!(step = await iterator.next()).done) {
    console.log("value", step.value);
  }
  console.log("final", step.value);

  for await (const value of catching()) {
    console.log("catching", value);
  }

  const aborted = outer();
  console.log("first", (await aborted.next()).value);
  const returned = await aborted.return(7);
  console.log("returned", returned.value, returned.done);
  console.log("after-return", (await aborted.next()).done);
})();
