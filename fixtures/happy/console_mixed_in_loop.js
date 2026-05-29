// Mixed console levels inside loop — verify no state pollution between iterations.
for (let i = 0; i < 3; i++) {
  console.log("log", i);
  console.warn("warn", i);
  console.info("info", i);
}
console.log("done");
