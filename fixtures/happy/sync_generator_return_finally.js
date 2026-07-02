function* gen() {
  try {
    yield "body";
  } finally {
    yield "cleanup";
  }
}

const g = gen();
const r1 = g.next();
console.log("n1", r1.value, r1.done);
const r2 = g.return("done");
console.log("r2", r2.value, r2.done);
const r3 = g.next();
console.log("n3", r3.value, r3.done);
const r4 = g.next();
console.log("n4", r4.value, r4.done);
