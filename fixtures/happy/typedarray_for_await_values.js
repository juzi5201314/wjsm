async function run() {
  var ta = new Uint8Array([1, 2, 3]);
  var iter = ta.values();
  ta[1] = 8;
  var result = [];
  for await (var x of iter) {
    result.push(x);
  }
  console.log(result.join(","));
}
run();
