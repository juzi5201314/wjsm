// Error inside timer callback — documents current host-timer behavior.
// Timer callbacks fire; a thrown callback error does not stop later timer callbacks.
console.log("main-start");

setTimeout(() => {
  console.log("before-throw");
  throw new Error("timer-callback-error");
}, 0);

setTimeout(() => {
  console.log("after-error-timer");
}, 0);

console.log("main-end");
