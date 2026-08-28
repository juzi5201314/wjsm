// AbortSignal 的 EventTarget 身份与 abort 事件：原型链、全局构造器身份、
// addEventListener/removeEventListener/onabort、一次性派发、重复 abort 不重发。
// 输出与 Node v22 逐字节对拍。
const controller = new AbortController();
const signal = controller.signal;

// 全局身份：AbortSignal/EventTarget/Event 均为全局构造器
console.log(typeof AbortSignal, typeof EventTarget, typeof Event);
console.log(globalThis.AbortSignal === AbortSignal, globalThis.EventTarget === EventTarget);
console.log(AbortSignal.name, AbortSignal.length, EventTarget.name, EventTarget.length, Event.name, Event.length);

// 原型链：signal → AbortSignal.prototype → EventTarget.prototype → Object.prototype
console.log(Object.getPrototypeOf(signal) === AbortSignal.prototype);
console.log(Object.getPrototypeOf(AbortSignal.prototype) === EventTarget.prototype);
console.log(Object.getPrototypeOf(EventTarget.prototype) === Object.prototype);
console.log(signal instanceof AbortSignal, signal instanceof EventTarget);
console.log(signal.constructor === AbortSignal, AbortSignal.prototype.constructor === AbortSignal);
console.log(String(signal), String(new EventTarget()));
console.log(Object.prototype.toString.call(signal));

// EventTarget 方法继承自 EventTarget.prototype，而不是实例自有属性
console.log(typeof signal.addEventListener, typeof signal.removeEventListener, typeof signal.dispatchEvent);
console.log(signal.addEventListener === EventTarget.prototype.addEventListener);
console.log(Object.getOwnPropertyNames(signal).length === 0);

// new AbortSignal() 非法构造
try {
  new AbortSignal();
} catch (e) {
  console.log(e.name + ": " + e.message);
}

// abort 事件：监听器与 onabort 各按注册顺序派发一次，事件对象字段可观察
const order = [];
signal.addEventListener("abort", (e) => {
  order.push("listener:" + e.type + ":" + (e.target === signal) + ":" + (e.currentTarget === signal) + ":" + e.isTrusted);
});
signal.onabort = (e) => order.push("onabort:" + e.type);
console.log(typeof signal.onabort);
controller.abort("first reason");
console.log(JSON.stringify(order), signal.aborted, signal.reason);

// 重复 abort：不重发事件、reason 不变
controller.abort("second reason");
console.log(JSON.stringify(order), signal.reason);

// abort 后再注册监听器：不追发
signal.addEventListener("abort", () => order.push("late"));
console.log(JSON.stringify(order));

// removeEventListener 在派发前移除则不触发
const c2 = new AbortController();
const seen = [];
const removed = () => seen.push("removed");
c2.signal.addEventListener("abort", removed);
c2.signal.addEventListener("abort", () => seen.push("kept"));
c2.signal.removeEventListener("abort", removed);
c2.abort();
console.log(JSON.stringify(seen), c2.signal.reason.name, c2.signal.reason.message);

// onabort 置 null 后不再触发；once 选项只触发一次
const c3 = new AbortController();
let onabortRuns = 0;
c3.signal.onabort = () => onabortRuns++;
c3.signal.onabort = null;
console.log(c3.signal.onabort);
c3.signal.addEventListener("abort", () => onabortRuns += 10, { once: true });
c3.abort();
console.log(onabortRuns, c3.signal.aborted);

// throwIfAborted：未 abort 返回 undefined，abort 后抛 reason
const c4 = new AbortController();
console.log(c4.signal.throwIfAborted());
c4.abort("boom reason");
try {
  c4.signal.throwIfAborted();
} catch (e) {
  console.log("threw: " + e);
}
