// #3: conditional break inside for-of must not trap
// Array (no custom return)
let out = [];
for (const x of [0, 1, 2, 3]) {
  if (x === 2) break;
  out.push(x);
}
console.log("array:", JSON.stringify(out));

// Custom iterator with return()
function makeIter() {
  let i = 0;
  return {
    [Symbol.iterator]() {
      return {
        next() { let v = i; i = i + 1; return { value: v, done: v > 5 }; },
        return() { console.log("closed"); return { done: true }; }
      };
    }
  };
}
let out2 = [];
for (const y of makeIter()) {
  if (y === 2) break;
  out2.push(y);
}
console.log("custom:", JSON.stringify(out2));

// for-in conditional break
let out3 = [];
for (const k in { a: 1, b: 2, c: 3 }) {
  if (k === "c") break;
  out3.push(k);
}
console.log("for-in:", JSON.stringify(out3));
