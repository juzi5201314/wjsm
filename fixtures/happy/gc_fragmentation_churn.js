// 长期分配/释放碎片压力测试（issue #332）。
//
// 验证 non-moving mark-sweep + 尾部空间回收在长期 churn 下的外部碎片控制：
// 1. 分配大量临时对象（不同 size class）→ 释放 → 再分配，制造碎片
// 2. 长期运行后堆不膨胀失控（尾部空间回收使 heap_ptr 回退）
// 3. 存活对象跨多轮 GC 完好
//
// 此测试不直接断言内存字节数（JS 无 performance.memory API），
// 而是通过验证正确性 + 不 OOM trap 来间接验证碎片治理有效。
// Rust 侧的 heap_governance 单元测试覆盖尾部回收的精确语义。

// 存活对象：跨整个测试存活，验证不被误回收
const survivor = { tag: "alive", data: [1, 2, 3, 4, 5] };

// 阶段 1：大量不同 size class 的分配-释放 churn
// 每轮分配 100 个对象（覆盖 cap 1..10 的 size class），不保留引用 → 全部可回收
for (let round = 0; round < 50; round++) {
  for (let i1 = 0; i1 < 100; i1++) {
    // 不同 capacity → 不同 size class（16 + cap*32 字节）
    const cap = (i1 % 10) + 1;
    const tmp = { a: i1, b: i1 + 1, c: i1 + 2 };
    for (let j1 = 0; j1 < cap; j1++) {
      tmp["k" + j1] = j1;
    }
  }
}

// 阶段 2：数组 churn（数组 size class: 16 + len*8）
const arrays = [];
for (let r2 = 0; r2 < 50; r2++) {
  for (let i2 = 0; i2 < 100; i2++) {
    const len = (i2 % 20) + 1;
    const arr = new Array(len);
    for (let j2 = 0; j2 < len; j2++) {
      arr[j2] = j2 * r2;
    }
  }
}

// 阶段 3：混合 size class 的交替分配（制造碎片）
for (let r3 = 0; r3 < 30; r3++) {
  // 大对象 + 小对象交替
  const big = { data: r3 };
  const small = { x: r3 };
  const big2 = { data: r3 };
  const small2 = { y: r3 };
}

// 验证存活对象完好
console.log(survivor.tag);
console.log(survivor.data.join(","));
console.log(survivor.data.reduce((a, b) => a + b, 0));
