let savedController;
const stream = new ReadableStream({
  type: "bytes",
  start(controller) { savedController = controller; }
});
const reader = stream.getReader({ mode: "byob" });
const view = new Uint8Array(4);
reader.read(view).then(r => {
  console.log("done", r.done);
  console.log("len", r.value.length);
  console.log("byte0", r.value[0]);
});
// 跨多次内存扩展阈值（默认 ~1600 对象分配），验证 pending view/promise 在
// 内存增长后仍保持有效。使用对象字面量分配触发 memory.grow。
let sink = [];
for (let i = 0; i < 10000; i++) sink.push({ x: i });
const req = savedController.byobRequest;
req.view[0] = 88;
req.respond(1);
