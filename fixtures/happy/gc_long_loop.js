// GC 长循环验收：多次 GC 周期不 OOM，死对象被回收。
// 每轮创建临时对象（不保留引用 → 可回收），累计分配远超单次内存。
// 不依赖数组扩容（数组有 capacity 上限），纯靠对象轮换验证 GC。
let total = 0;
for (let i = 0; i < 200000; i++) {
  // 临时对象：分配后立即丢弃，GC 应回收
  const tmp = { x: i, y: i + 1 };
  total += tmp.x;
}
// total = sum(0..199999) = 19999900000（超 int32，但 JS number 是 f64）
console.log("done", total > 0);
