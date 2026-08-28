// async generator `.return()` 在 yield 悬挂点同样以 return completion 展开
// 迭代器保护区：内层 for-of 的 IteratorClose 先于外层 finally 执行。
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
  console.log(JSON.stringify(await it.return(42)));
  console.log(JSON.stringify(await it.next()));
}
main();
