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
// 跨多次 GC 阈值（默认 1000 分配），验证 pending view/promise 被 mark 为 root。
// 使用字符串连接（堆分配）触发 GC，避免对象字面量在同步路径上触发已知
// 的异步再入问题。
let sink = "";
for (let i = 0; i < 10000; i++) sink += "x";
const req = savedController.byobRequest;
req.view[0] = 88;
req.respond(1);
