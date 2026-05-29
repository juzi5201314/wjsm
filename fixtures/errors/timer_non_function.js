// Non-function passed to setTimeout — current implementation behavior documented.
console.log("start");

try {
  setTimeout("not a function", 0);
  console.log("no-sync-throw");
} catch (e) {
  console.log("sync-throw:", e.message);
}

console.log("end");
