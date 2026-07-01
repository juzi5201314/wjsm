// Test: writer.close waits for TransformStream flush before resolving close promises
const transform = new TransformStream({
  flush(controller) {
    console.log("flush");
    controller.enqueue("tail");
  }
});
const writer = transform.writable.getWriter();
writer.closed.then(() => {
  console.log("writer closed");
});
writer.close().then(() => {
  console.log("close done");
});
transform.readable.getReader().read().then(result => {
  console.log(result.value, result.done);
});
