function* gen() {
  yield 1;
  yield 2;
}

const g = gen();
const r1 = g.next();
console.log("n1", r1.value, r1.done);
const r2 = g.return(99);
console.log("r2", r2.value, r2.done);
const r3 = g.next();
console.log("n3", r3.value, r3.done);
