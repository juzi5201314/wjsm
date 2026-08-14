let a = [10, 20, 30];
let r1 = a.reduce((acc, x, i, arr) => {
  if (i === 0 && arr.length !== 3) throw new Error("bad arr");
  return acc + x;
}, 0);
console.log(r1);
let r2 = a.reduceRight((acc, x, i) => acc + x * (i + 1), 0);
console.log(r2);
