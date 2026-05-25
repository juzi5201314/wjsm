async function run() {
  let obj = {
    [Symbol.asyncIterator]() {
      let i = 0;
      return {
        async next() {
          if (i < 3) {
            i = i + 1;
            return { value: i, done: false };
          }
          return { value: undefined, done: true };
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
