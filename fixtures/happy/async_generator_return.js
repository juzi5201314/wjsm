async function* gen() {
  yield "first";
  return "done";
}

let g = gen();
g.next().then((result) => console.log(result.value));
g.next().then((result) => console.log(result.value));
g.next().then((result) => console.log(result.done));
