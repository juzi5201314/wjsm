let resolveA;
let rejectB;
let p1 = new Promise(resolve => { resolveA = resolve; });
let p2 = new Promise((resolve, reject) => { rejectB = reject; });

Promise.allSettled([p1, p2]).then(results => {
  console.log(results[0].status);
  console.log(results[0].value);
  console.log(results[1].status);
  console.log(results[1].reason);
});

queueMicrotask(() => {
  rejectB(2);
  resolveA(1);
});
