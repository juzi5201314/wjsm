// Test: ReadableStream.pipeTo() transfers chunks into a TransformStream writable
const source = new ReadableStream({
  start(controller) {
    controller.enqueue("alpha");
    controller.enqueue("beta");
    controller.close();
  }
});
const transform = new TransformStream({
  transform(chunk, controller) {
    controller.enqueue(chunk.toUpperCase());
  }
});
const reader = transform.readable.getReader();
source.pipeTo(transform.writable).then(() => {
  console.log("pipeTo done");
});
reader.read().then(r1 => {
  console.log(r1.value);
  return reader.read();
}).then(r2 => {
  console.log(r2.value);
  return reader.read();
}).then(r3 => {
  console.log(r3.done);
});
