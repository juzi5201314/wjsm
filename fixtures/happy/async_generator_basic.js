async function* gen() {
  yield 1;
  yield 2;
}

let g = gen();
g.next().then((result) => console.log(result.value));
g.next().then((result) => console.log(result.value));
g.next().then((result) => console.log(result.done));
