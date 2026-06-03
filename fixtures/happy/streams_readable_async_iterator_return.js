// Test: ReadableStream async iterator return() — 提前释放 reader
const stream = new ReadableStream({
  start(controller) {
    controller.enqueue("x");
    controller.enqueue("y");
  }
});

// 获取 async iterator
const iter = stream[Symbol.asyncIterator]();
console.log(stream.locked);          // true

// 读取一个 chunk 后调用 return()
iter.next().then(r1 => {
  console.log(r1.done);             // false
  console.log(r1.value);            // x
  return iter.return();
}).then(r2 => {
  console.log(r2.done);             // true
  console.log(r2.value);            // undefined
  // return() 后流不再锁定
  console.log(stream.locked);       // false
});
