// Issue #390：特化 activation 跨多轮 GC、闭包环境、类型漂移与版本淘汰仍保持 entry 存活。
function makeAdder(base) {
  const environment = { base, calls: 0 };
  return function add(value) {
    environment.calls += 1;
    return environment.base + value + environment.calls;
  };
}

const add = makeAdder(10);
function invoke(value) {
  return add(value);
}

let checksum = 0;
for (let round = 0; round < 3; round++) {
  for (let i = 0; i < 120; i++) {
    checksum += invoke(i);
    const garbage = { round, i, nested: { value: i + round } };
    if (garbage.nested.value < 0) console.log("unreachable");
  }
  gc();
}
console.log("checksum", checksum);
console.log("number", invoke(1));
console.log("string", invoke("x"));
console.log("object", invoke({ valueOf() { return 2; } }));
gc();
console.log("after-gc", invoke(3));
