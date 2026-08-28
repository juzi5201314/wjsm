// generator `.throw()` 在 yield 悬挂点必须以 throw completion 交错展开迭代
// 保护区与 finally：外围 for-of 的 IteratorClose（§7.4.11，经 ForIn/
// OfBodyEvaluation）先于更外层 finally 执行（内层优先），而非先跑全部
// finalizer 再统一关迭代器。
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
function* g() {
  try {
    for (const x of makeIter("iter")) {
      yield x;
    }
  } finally {
    console.log("fn finally");
  }
}
const it = g();
console.log(JSON.stringify(it.next()));
try {
  it.throw(new Error("boom"));
} catch (e) {
  console.log("caught: " + e.message);
}
console.log(JSON.stringify(it.next()));
