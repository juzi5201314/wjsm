// Test: ReadableStream[Symbol.asyncIterator]() — for-await-of 协议
const stream = new ReadableStream({
  start(controller) {
    controller.enqueue("a");
    controller.enqueue("b");
    controller.close();
  }
});

console.log(typeof stream);          // object
console.log(stream.locked);          // false

// 获取 async iterator
const iter = stream[Symbol.asyncIterator]();
console.log(typeof iter);            // object
console.log(typeof iter.next);       // function
console.log(typeof iter.return);     // function

// 流已被锁定（iterator 持有 reader）
console.log(stream.locked);          // true

// next() 返回 Promise<{done, value}>
iter.next().then(r1 => {
  console.log(r1.done);             // false
  console.log(r1.value);            // a
  return iter.next();
}).then(r2 => {
  console.log(r2.done);             // false
  console.log(r2.value);            // b
  return iter.next();
}).then(r3 => {
  console.log(r3.done);             // true
  console.log(r3.value);            // undefined
});
