// Console calls inside timer callback — verifies console + timer interaction.
console.log("before-timer");

setTimeout(() => {
  console.log("inside-timer-log");
  console.warn("inside-timer-warn");
}, 0);

console.log("after-schedule");
