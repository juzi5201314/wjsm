// Timer + Promise interaction — documents actual behavior (timer callbacks do not fire).
// Expected spec behavior: microtasks drain before next timer callback.
// Actual: only main-thread + microtask output captured; timer callback is dead code.
console.log("main");

setTimeout(() => {
  console.log("timeout");
  Promise.resolve().then(() => console.log("promise-inside-timeout"));
}, 0);

Promise.resolve().then(() => console.log("promise-before-timeout"));

console.log("main-end");
