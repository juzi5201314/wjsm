// generator `.return()` 在 yield 悬挂点必须以 return completion 展开迭代器
// 保护区：外围 for-of 的 IteratorClose（§7.4.11，经 ForIn/OfBodyEvaluation）
// 先于更外层 finally 执行（内层优先），close 无参调用 return()。
function makeIter(tag) {
  return {
    [Symbol.iterator]() {
      let i = 0;
      return {
        next() { i += 1; return { value: i, done: i > 3 }; },
        return(v) { console.log(tag + " closed, arg=" + String(v)); return { done: true }; },
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
console.log(JSON.stringify(it.return(99)));
console.log(JSON.stringify(it.next()));
