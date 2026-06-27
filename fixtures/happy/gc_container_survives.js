// 存活于变量中的容器（数组/对象）必须跨 GC 存活。
// 变量持有的 handle 在 per-ValueId liveness 中是空洞（StoreVar 无 ValueId def、
// LoadVar 无 use），若 GC safepoint 不 spill 变量 local，容器被 sweep → 读到 undefined。
const arr = [10, 20, 30];
const obj = { a: 1, b: 2 };

// 触发多轮阈值 GC：分配远超 GC 阈值（1000）的临时对象（不保留引用 → 可回收）。
// 5000 次分配 ≈ 5 轮 GC，足以验证容器跨多轮 GC 存活；持续分配压力另见
// wjsm-runtime 的 bench_gc_sustained_allocation。
for (let i = 0; i < 5000; i++) {
  const tmp = { x: i, y: i + 1 };
}

// GC 后变量持有的容器内容必须完好。
console.log(arr[0], arr[1], arr[2]);
console.log(obj.a, obj.b);
