console.log("sync");
queueMicrotask(function() {
  console.log("micro");
});
console.log("sync2");
