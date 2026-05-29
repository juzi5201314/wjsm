// Error inside timer callback — documents actual behavior.
// NOTE: Timer callbacks do not currently fire, so this fixture produces no error output.
// The entire timer body is dead code; exit=0 because main-thread completes normally.
console.log("main-start");

setTimeout(() => {
  console.log("before-throw");
  throw new Error("timer-callback-error");
}, 0);

setTimeout(() => {
  console.log("after-error-timer");
}, 0);

console.log("main-end");
