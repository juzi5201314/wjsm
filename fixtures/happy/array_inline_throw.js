try {
  let b = [1, 2, 3].map(x => { if (x === 2) throw new Error("boom"); return x; });
  console.log("unreached", b);
} catch (e) {
  console.log("caught", e.message);
}
console.log("after");
