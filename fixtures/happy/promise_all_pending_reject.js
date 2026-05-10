let resolveA;
let rejectB;
let p1 = new Promise(resolve => { resolveA = resolve; });
let p2 = new Promise((resolve, reject) => { rejectB = reject; });

Promise.all([p1, p2])
  .then(() => console.log("fulfilled"))
  .catch(reason => console.log(reason));

queueMicrotask(() => {
  resolveA(1);
  rejectB("bad");
});
