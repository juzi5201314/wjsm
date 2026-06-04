// Test: ReadableStream.pipeThrough() accepts a readable/writable pair
const source = new ReadableStream({
  start(controller) {
    controller.enqueue("red");
    controller.enqueue("blue");
    controller.close();
  }
});
const transform = new TransformStream({
  transform(chunk, controller) {
    controller.enqueue(chunk + "!");
  }
});
const piped = source.pipeThrough({ readable: transform.readable, writable: transform.writable });
const reader = piped.getReader();
reader.read().then(r1 => {
  console.log(r1.value);
  return reader.read();
}).then(r2 => {
  console.log(r2.value);
  return reader.read();
}).then(r3 => {
  console.log(r3.done);
});
