let resolveA;
let resolveB;
let p1 = new Promise(resolve => { resolveA = resolve; });
let p2 = new Promise(resolve => { resolveB = resolve; });

Promise.all([p1, p2]).then(values => {
  console.log(values[0]);
  console.log(values[1]);
});

queueMicrotask(() => {
  resolveB(2);
  resolveA(1);
});
