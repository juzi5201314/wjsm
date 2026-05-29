// Interval clears itself from inside callback — documents actual behavior.
// NOTE: Runtime emits "Internal error: timer event loop exceeded max iterations"
// because timer callbacks do not currently fire, so clearInterval() is never
// reached. The repeating timer is rescheduled indefinitely, triggering the
// max-iterations guard.
console.log("start");

let count = 0;
let id;
id = setInterval(() => {
  count++;
  console.log("tick", count);
  if (count >= 1) {
    clearInterval(id);
  }
}, 0);

console.log("end");
