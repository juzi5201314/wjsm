async function run() {
  for await (let x of 42) {
    console.log(x);
  }
}
run();
