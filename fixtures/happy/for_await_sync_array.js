async function run() {
  let result = [];
  for await (let x of [1, 2, 3]) {
    result.push(x);
  }
  console.log(result.join(","));
}
run();
