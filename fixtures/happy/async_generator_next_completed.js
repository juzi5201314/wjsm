async function* gen() {
  yield 1;
}
let g = gen();
g.next().then((r1) => {
  console.log(r1.value);
  console.log(r1.done);
  return g.next();
}).then((r2) => {
  console.log(r2.value);
  console.log(r2.done);
  return g.next();
}).then((r3) => {
  console.log(r3.value);
  console.log(r3.done);
});
