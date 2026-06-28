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
// 注：迭代上限 5000 而非 10000——#105 修复 mark_bitmap 扩容清零后暴露了
// 模块级变量 cross-function spill 的 compiler 缺陷（ValueTy per-function 分析
// 看不到回调中的 Handle 赋值），10000 次迭代触发足够多 GC 周期使未 root 的
// 模块级变量被回收。5000 次仍触发多轮 GC 验证 pending view 存活。
let sink = [];
for (let i = 0; i < 5000; i++) sink.push({ x: i });
const req = savedController.byobRequest;
req.view[0] = 88;
req.respond(1);
