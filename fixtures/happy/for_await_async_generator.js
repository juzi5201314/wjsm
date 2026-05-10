async function run() {
  async function* gen() {
    yield 1;
    yield 2;
  }

  for await (let value of gen()) {
    console.log(value);
  }

  console.log("done");
}

run();
