// Test execution order: main → microtask → timer → microtask (produced by timer)
console.log("main-1");

setTimeout(() => {
  console.log("timer-1");
  queueMicrotask(() => console.log("microtask-from-timer"));
}, 0);

Promise.resolve().then(() => console.log("microtask-1"));

console.log("main-2");
