// Test: pipeTo keeps pumping chunks enqueued after pipeTo starts
let sourceController;
const source = new ReadableStream({
  start(controller) {
    sourceController = controller;
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
sourceController.enqueue("late");
sourceController.close();
reader.read().then(result => {
  console.log(result.value, result.done);
});
