let rejectA;
let rejectB;
let p1 = new Promise((resolve, reject) => { rejectA = reject; });
let p2 = new Promise((resolve, reject) => { rejectB = reject; });

Promise.any([p1, p2]).catch(err => {
  console.log(err.name);
  console.log(err.errors.length);
  console.log(err.errors[0]);
  console.log(err.errors[1]);
});

queueMicrotask(() => {
  rejectB(2);
  rejectA(1);
});
