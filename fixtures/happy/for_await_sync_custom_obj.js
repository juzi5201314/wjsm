async function run() {
  const obj = {
    [Symbol.iterator]() {
      let i = 0;
      return {
        next() {
          i++;
          return { value: i, done: i > 2 };
        }
      };
    }
  };
  let result = [];
  for await (let x of obj) {
    result.push(x);
  }
  console.log(result.join(","));
}
run();