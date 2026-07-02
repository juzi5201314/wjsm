function* gen() {
  yield 1;
}

const a = gen();
let r = a.return(7);
console.log("start-return", r.value, r.done);
r = a.next();
console.log("after-return", r.value, r.done);

const b = gen();
try {
  b.throw(new Error("x"));
} catch (error) {
  console.log("start-throw", error.message);
}

const c = gen();
r = c.next();
console.log("first", r.value, r.done);
r = c.next();
console.log("completed", r.value, r.done);
try {
  c.throw(new Error("done"));
} catch (error) {
  console.log("completed-throw", error.message);
}
