// Nested timers: setTimeout schedules another setTimeout.
console.log("start");

setTimeout(() => {
  console.log("outer");
  setTimeout(() => {
    console.log("inner");
  }, 0);
}, 0);

console.log("end");
