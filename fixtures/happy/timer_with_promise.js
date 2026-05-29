// Timer callback + Promise microtask priority: microtasks drain before next timer.
console.log("main");

setTimeout(() => {
  console.log("timeout");
  Promise.resolve().then(() => console.log("promise-inside-timeout"));
}, 0);

Promise.resolve().then(() => console.log("promise-before-timeout"));

console.log("main-end");
