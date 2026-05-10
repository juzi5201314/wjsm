async function* gen(input) {
  let awaited = await Promise.resolve(input + 1);
  yield awaited;
  yield input + 2;
}

let g = gen(4);
g.next().then((result) => console.log(result.value));
g.next().then((result) => console.log(result.value));
