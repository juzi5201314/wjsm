// Clear timeout before it fires — callback must NOT execute.
console.log("start");

const id = setTimeout(() => {
  console.log("SHOULD-NOT-EXECUTE");
}, 0);

clearTimeout(id);

console.log("end");
