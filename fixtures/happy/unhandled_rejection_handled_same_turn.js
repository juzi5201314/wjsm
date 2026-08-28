// 微任务检查点之前挂上的 handler 不得误报（按规范时机）：
// 1) 同步 .catch；2) 同一轮微任务级联内挂 .catch；3) async 结果被 .catch。
const a = Promise.reject(new Error("a"));
a.catch((e) => console.log("caught-a", e.message));

const b = Promise.reject(new Error("b"));
Promise.resolve().then(() => {
  b.catch((e) => console.log("caught-b", e.message));
});

async function boom() {
  throw new Error("c");
}
boom().catch((e) => console.log("caught-c", e.message));
console.log("main-done");
