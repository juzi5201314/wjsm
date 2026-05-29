// Multiple clearTimeout on same id — must be idempotent.
console.log("start");

const id = setTimeout(() => console.log("SHOULD-NOT-RUN"), 0);

clearTimeout(id);
clearTimeout(id);
clearTimeout(id);

console.log("end");
