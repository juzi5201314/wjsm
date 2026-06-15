// GC safepoint spill with multiple live handles across a call to an allocating function.
function inner() {
  return { v: 42 };
}
function outer() {
  const a = { x: 1 };
  const b = { y: 2 };
  const c = { z: 3 };
  // inner() allocates and may trigger GC; a, b, c must stay live via spill.
  const d = inner();
  return [a, b, c, d];
}
const arr = outer();
console.log(arr[0].x + arr[1].y + arr[2].z + arr[3].v);
