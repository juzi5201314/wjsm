// Interval clears itself from inside callback.
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
