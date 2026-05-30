// Console calls inside timer callback — documents actual behavior.
// Timer callbacks now fire correctly.
console.log("before-timer");

setTimeout(() => {
  console.log("inside-timer-log");
  console.warn("inside-timer-warn");
}, 0);

console.log("after-schedule");
