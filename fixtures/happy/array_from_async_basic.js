// Array.fromAsync（proposal-array-from-async，Node v22 形态）基本路径：
// 异步迭代器 / 同步可迭代（CreateAsyncFromSyncIterator）/ array-like 回退。
// 各场景以 await 串行化，避免跨场景微任务交错。
const show = (v) => JSON.stringify(v);

async function main() {
  // 属性形态。
  console.log(Array.fromAsync.name, Array.fromAsync.length, typeof Array.fromAsync);

  // 同步可迭代：数组（含 promise 元素解包）、字符串、Set、Map。
  console.log("array", show(await Array.fromAsync([1, Promise.resolve(2), 3])));
  console.log("string", show(await Array.fromAsync("ab")));
  console.log("set", show(await Array.fromAsync(new Set([3, 3, 4]))));
  console.log("map", show((await Array.fromAsync(new Map([[1, "x"]]))).map((p) => show(p))));

  // 同步迭代器逐个产出 promise：按序解包。
  const syncGen = { *[Symbol.iterator]() { yield Promise.resolve("p1"); yield "v2"; } };
  console.log("syncGen", show(await Array.fromAsync(syncGen)));

  // 同步迭代器 done 结果的 value 为 promise：仍会等待但值弃用。
  const doneP = {
    [Symbol.iterator]() {
      let i = 0;
      return { next() { return i++ ? { done: true, value: Promise.resolve(99) } : { done: false, value: 1 }; } };
    },
  };
  console.log("syncDoneP", show(await Array.fromAsync(doneP)));

  // 纯异步迭代器（手写 next 返回 promise）。
  const asyncSrc = {
    [Symbol.asyncIterator]() {
      let i = 0;
      return { next() { return Promise.resolve(i < 3 ? { done: false, value: i++ * 10 } : { done: true }); } };
    },
  };
  console.log("asyncSrc", show(await Array.fromAsync(asyncSrc)));

  // 异步生成器源。
  async function* agen() { yield "g1"; yield "g2"; }
  console.log("agen", show(await Array.fromAsync(agen())));

  // mapfn：同步与异步、index 实参、thisArg。
  console.log("map", show(await Array.fromAsync([1, 2, 3], (v, i) => v * 2 + i)));
  console.log("mapAsync", show(await Array.fromAsync([1, 2], async (v, i) => v * 10 + i)));
  console.log("thisArg", show(await Array.fromAsync([1], function (v) { return this.base + v; }, { base: 10 })));

  // array-like 回退：缺元素为 undefined、promise 元素解包、length 强转。
  console.log("arrlike", show(await Array.fromAsync({ length: 3, 0: Promise.resolve("a"), 1: "b" })));
  console.log("lenValueOf", show(await Array.fromAsync({ length: { valueOf() { return 2; } }, 0: "x", 1: "y" })));
  console.log("lenNeg", show(await Array.fromAsync({ length: -3 })));
  console.log("primitive", show(await Array.fromAsync(5)));

  // 结果是真数组。
  const out = await Array.fromAsync([7]);
  console.log("isArray", Array.isArray(out), out.length);

  // 提取为独立函数调用（动态属性访问路径）。
  const detached = Array.fromAsync;
  console.log("detached", show(await detached([Promise.resolve(7), 8])));
}

main().then(() => console.log("done"));
