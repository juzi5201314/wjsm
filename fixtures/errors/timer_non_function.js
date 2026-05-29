// Non-function passed to setTimeout — current implementation behavior documented.
// NOTE: console.log() inside try{} currently triggers catch due to a runtime bug
// (return value misdetected as exception). Separate try/catch blocks to isolate.
console.log("start");

try {
  setTimeout("not a function", 0);
} catch (e) {
  console.log("sync-throw:", e.message);
}

try {
  console.log("no-sync-throw");
} catch (e) {
  // console.log in try triggers catch — runtime bug
  console.log("log-caught-as-exception");
}

console.log("end");
