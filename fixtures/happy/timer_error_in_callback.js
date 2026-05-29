// Error inside timer callback — main execution must continue, error should be observable.
console.log("main-start");

setTimeout(() => {
  console.log("before-throw");
  throw new Error("timer-callback-error");
}, 0);

setTimeout(() => {
  console.log("after-error-timer");
}, 0);

console.log("main-end");
