async function run() {
  async function* gen() { yield 1; }
  let g = gen();
  console.log(typeof g.next);
  console.log(typeof g.return);
  console.log(typeof g.throw);
  console.log(typeof g[Symbol.asyncIterator]);
}
run();
