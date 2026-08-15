function assertEmptyReduce(name, reduce) {
  let caught = false;
  try {
    reduce();
  } catch (e) {
    caught = e instanceof TypeError;
  }
  console.log(name + ": " + caught);
}

assertEmptyReduce("array reduce", () => [].reduce((a, b) => a + b));
assertEmptyReduce("array reduceRight", () => [].reduceRight((a, b) => a + b));
assertEmptyReduce("typedarray reduce", () => new Uint8Array(0).reduce((a, b) => a + b));
assertEmptyReduce(
  "typedarray reduceRight",
  () => new Uint8Array(0).reduceRight((a, b) => a + b),
);
console.log("normal exit");
