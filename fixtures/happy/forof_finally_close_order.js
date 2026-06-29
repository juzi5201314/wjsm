// #145: for-of 在 try-finally 内时，abrupt completion 应内层优先展开 ——
// 迭代器（内层）先于 finally（外层）关闭。spec §8.5.5 IteratorClose 顺序。
var log = [];
function makeIter(onClose) {
  return {
    [Symbol.iterator]() {
      let i = 0;
      return {
        next() { return { value: i++, done: i > 3 }; },
        return() { onClose(); return { done: true }; },
      };
    },
  };
}
function forof_in_try() {
  try {
    for (const x of makeIter(() => log.push("close"))) { return 1; }
  } finally {
    log.push("finally");
  }
}
forof_in_try();
console.log(JSON.stringify(log));
