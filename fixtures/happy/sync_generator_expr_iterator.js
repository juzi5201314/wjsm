const makeGen = function* () {
  yield "expr";
};

const g = makeGen();
console.log("self", g[Symbol.iterator]() === g);
const r1 = g.next();
console.log("n1", r1.value, r1.done);
const r2 = g.next();
console.log("n2", r2.value, r2.done);
