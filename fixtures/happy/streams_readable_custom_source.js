// Test: ReadableStream constructor with user-defined underlyingSource
let ctrl;
const stream = new ReadableStream({
  start(controller) {
    ctrl = controller;
    controller.enqueue("hello");
    controller.enqueue("world");
    controller.close();
  }
});

console.log(typeof stream);          // object
console.log(stream.locked);          // false

const reader = stream.getReader();
console.log(stream.locked);          // true

// First read via await
const r1 = await reader.read();
console.log(r1.done);                // false
console.log(r1.value);               // hello

// Second read via .then() — avoids 2+ sequential await in async context
reader.read().then(r2 => {
  console.log(r2.done);              // false
  console.log(r2.value);             // world
});

// Third read via .then() — verifies close returns done:true, value:undefined
reader.read().then(r3 => {
  console.log(r3.done);             // true
  console.log(r3.value);            // undefined
});
