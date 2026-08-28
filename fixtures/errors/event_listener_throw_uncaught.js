// EventTarget 监听器异常：不中断本轮派发，同步代码继续执行，
// 异常延后到下一 tick 作为未捕获错误重抛（对齐 Node 的 kHybridDispatch 行为）。
const et = new EventTarget();
et.addEventListener("x", () => {
  throw new Error("listener boom");
});
et.addEventListener("x", () => console.log("second ran"));
console.log("dispatch returned:", et.dispatchEvent(new Event("x")));

const controller = new AbortController();
controller.signal.addEventListener("abort", () => {
  throw new Error("abort boom");
});
controller.signal.addEventListener("abort", () => console.log("abort second ran"));
controller.abort();
console.log("end of script");
