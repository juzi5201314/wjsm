// Test: ReadableStream byte stream with BYOB reader copies bytes into the supplied view
const stream = new ReadableStream({
  type: "bytes",
  start(controller) {
    console.log("byobRequest null:", controller.byobRequest === null);
    controller.enqueue(new Uint8Array([65, 66, 67]));
    controller.close();
  }
});

console.log("locked before:", stream.locked);
const reader = stream.getReader({ mode: "byob" });
console.log("locked after:", stream.locked);
const view = new Uint8Array(4);
reader.read(view).then(result => {
  console.log("done:", result.done);
  console.log("value length:", result.value.length);
  console.log("value byte0:", result.value[0]);
  console.log("value byte1:", result.value[1]);
  console.log("view byte2:", view[2]);
  return reader.read(new Uint8Array(1));
}).then(second => {
  console.log("second done:", second.done);
});
