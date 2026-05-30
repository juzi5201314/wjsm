// Test that exceptions in timer callbacks don't stop the event loop
setTimeout(() => {
  console.log("timer-1-start");
  throw new Error("error in timer-1");
}, 0);

setTimeout(() => {
  console.log("timer-2");
}, 0);

console.log("main");
