// Console calls inside timer callback — documents actual behavior.
// NOTE: Timer callbacks do not currently fire, so this only verifies console
// works in main-thread + microtask context. The timer callback is dead code.
console.log("before-timer");

setTimeout(() => {
  console.log("inside-timer-log");
  console.warn("inside-timer-warn");
}, 0);

console.log("after-schedule");
