async function* gen() {
  yield 1;
  yield 2;
  yield 3;
}
let g = gen();
g.next().then((r1) => {
  console.log(r1.value);
  console.log(r1.done);
  return g.return("early");
}).then((ret) => {
  console.log(ret.value);
  console.log(ret.done);
  return g.next();
}).then((r3) => {
  console.log(r3.value);
  console.log(r3.done);
});
