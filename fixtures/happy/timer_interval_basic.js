// setInterval + immediate clear — callback may execute 0 or 1 times (implementation dependent).
// We only verify it does not crash and produces deterministic output.
console.log("start");

const id = setInterval(() => {
  console.log("interval-tick");
}, 0);

clearInterval(id);

console.log("end");
