async function run() {
  async function* gen() { yield 1; }
  let g = gen();
  let asyncIterMethod = g[Symbol.asyncIterator];
  let result = asyncIterMethod.call(g);
  console.log(result === g);
}
run();
