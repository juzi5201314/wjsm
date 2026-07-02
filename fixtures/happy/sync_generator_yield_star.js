function* inner() {
  yield 1;
  return 2;
}

function* outer() {
  yield "start";
  const delegated = yield* inner();
  yield delegated + 3;
  yield* [6, 7];
  return "done";
}

const g = outer();
let r = g.next();
console.log("n1", r.value, r.done);
r = g.next();
console.log("n2", r.value, r.done);
r = g.next();
console.log("n3", r.value, r.done);
r = g.next();
console.log("n4", r.value, r.done);
r = g.next();
console.log("n5", r.value, r.done);
r = g.next();
console.log("n6", r.value, r.done);
