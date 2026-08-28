// 未处理的 Promise.reject 必须在微任务队列排空后被致命报告
// （Node --unhandled-rejections=throw 默认语义）：打印 reason 并以运行时错误码退出。
// 第二个 rejection 不再报告——进程在第一个报告处终止（与 Node 一致）。
// gc() 验证 reason 文本在 promise/Error 对象被回收后仍可报告（Issue #164 回归）。
Promise.reject(new Error("boom"));
Promise.reject("plain");
gc();
console.log("main-done");
