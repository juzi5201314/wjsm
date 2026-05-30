// clearTimeout inside a callback
console.log("main-1");

const id = setTimeout(() => {
  console.log("should-not-run");
}, 0);

setTimeout(() => {
  console.log("clearing");
  clearTimeout(id);
}, 0);

console.log("main-2");
