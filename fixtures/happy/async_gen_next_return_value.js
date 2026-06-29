// #1: queued next() + return(v) must preserve v via resume-return path
async function* gen() { yield 1; yield 2; yield 3; }
async function main() {
  const g = gen();
  // Queued before any microtask runs — exercises ResumeReturn path
  const p1 = g.next();
  const p2 = g.return(99);
  const r1 = await p1;
  const r2 = await p2;
  console.log("r1", r1.value, r1.done);
  console.log("r2", r2.value, r2.done);
  // After return, generator is completed
  const r3 = await g.next();
  console.log("r3", r3.value, r3.done);
}
main();
