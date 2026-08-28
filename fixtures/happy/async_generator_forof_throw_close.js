// async generator `.throw()` 在 yield 悬挂点同样以 throw completion 交错
// 展开：内层 for-of 的 IteratorClose 先于外层 finally 执行。
function makeIter(tag) {
  return {
    [Symbol.iterator]() {
      let i = 0;
      return {
        next() { i += 1; return { value: i, done: i > 3 }; },
        return() { console.log(tag + " closed"); return { done: true }; },
      };
    },
  };
}
async function* ag() {
  try {
    for (const x of makeIter("ag-iter")) {
      yield x;
    }
  } finally {
    console.log("ag finally");
  }
}
async function main() {
  const it = ag();
  console.log(JSON.stringify(await it.next()));
  try {
    await it.throw(new Error("ag boom"));
  } catch (e) {
    console.log("caught: " + e.message);
  }
  console.log(JSON.stringify(await it.next()));
}
main();
