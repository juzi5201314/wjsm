// G1 young GC 压力：长期存活对象持续指向新分配对象，同时制造短命对象 churn。
//
// 目标是覆盖 generational 路径最容易漏的两类引用：
// 1. 老对象/长期对象已有 property 指向新对象（dirty card / precise slot root）。
// 2. 固定长度数组元素指向新对象（support elem_set barrier event）。
//
// 本 fixture 避免动态扩容对象/数组，专注 young 引用更新语义；resize-abandoned
// 区域由 gc_fragmentation_churn 与 Rust heap_governance 单测覆盖。

const root = { tag: "root", latest: null, history: [null, null, null, null, null, null, null, null] };

for (let round = 0; round < 24; round++) {
  const child = { round, payload: [round, round + 1, round + 2] };
  root.latest = child;
  root.history[round % 8] = child;

  for (let i = 0; i < 48; i++) {
    const tmp = { i, round, nested: { value: i + round }, arr: [i, round, i + round] };
    if (tmp.nested.value === -1) {
      console.log("unreachable");
    }
  }
}

console.log(root.tag);
console.log(root.latest.round);
console.log(root.latest.payload.join(","));
console.log(root.history[7].round);
