// Test: TransformStream constructor
const ts = new TransformStream({
  transform(chunk, controller) {
    console.log("t:" + chunk);
    controller.enqueue(chunk.toUpperCase());
  },
  flush(controller) {
    console.log("flush");
    controller.enqueue("END");
  }
});
console.log(typeof ts);             // object
console.log(typeof ts.readable);    // object
console.log(typeof ts.writable);    // object
