// Trace after a throw — documents whether execution continues.
try {
  throw new Error("boom");
} catch (e) {
  console.log("caught");
  console.trace("trace-after-catch");
}
console.log("still-running");
