let pulls = 0;
const stream = new ReadableStream({
  type: "bytes",
  pull(controller) {
    pulls++;
    console.log("has request", controller.byobRequest !== null);
    const req = controller.byobRequest;
    const view = req.view;
    console.log("view length", view.length);
    view[0] = 65;
    view[1] = 66;
    req.respond(2);
    controller.close();
  }
});

const reader = stream.getReader({ mode: "byob" });
const buffer = new Uint8Array(8);
const first = await reader.read(buffer);
console.log("done", first.done);
console.log("value length", first.value.length);
console.log("bytes", first.value[0], first.value[1]);
const second = await reader.read(new Uint8Array(4));
console.log("second done", second.done);
console.log("pulls", pulls);
