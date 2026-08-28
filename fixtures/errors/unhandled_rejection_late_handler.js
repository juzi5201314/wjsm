// handler 挂载时机对齐 Node：微任务队列排空检查点在下一个宏任务之前执行，
// 排空后仍无 handler 即致命报告，setTimeout 回调没有机会再挂 catch。
const p = Promise.reject(new Error("late"));
setTimeout(() => {
  p.catch(() => console.log("too late"));
}, 0);
console.log("main-done");
