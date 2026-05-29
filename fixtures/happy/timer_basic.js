// Timer API basic test: setTimeout(fn, 0) with microtask interleaving.
// KNOWN-BROKEN: Timer callbacks do not currently fire — only main-thread + microtask output is captured.
// All timers use delay=0 for deterministic snapshot testing.
// Wall-clock time is NOT testable via snapshots — only execution order matters.
console.log("main-start");

setTimeout(() => {
  console.log("timeout-callback");
}, 0);

Promise.resolve().then(() => console.log("microtask-1"));
queueMicrotask(() => console.log("microtask-2"));

console.log("main-end");
