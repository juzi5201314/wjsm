// Test: TransformStream with writer and reader
const ts = new TransformStream({
  transform(chunk, controller) {
    controller.enqueue(chunk.toUpperCase());
  }
});
const writer = ts.writable.getWriter();
const reader = ts.readable.getReader();
writer.write("hello").then(() => {
  reader.read().then(r1 => {
    console.log(r1.value);           // HELLO
  });
  writer.write("world").then(() => {
    reader.read().then(r2 => {
      console.log(r2.value);         // WORLD
    });
    writer.close().then(() => {
      reader.read().then(r3 => {
        console.log(r3.done);        // true
      });
    });
  });
});
