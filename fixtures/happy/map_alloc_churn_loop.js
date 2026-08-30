// Map.set / keys / delete 分配 churn（对齐 alloc-churn bench 的核心路径）。
const CACHE = new Map();
let nextId = 0;
function work() {
  let kept = 0;
  for (let i = 0; i < 100; i++) {
    const obj = { id: nextId++, payload: "p" + i, values: [i, i * 2, i * 3] };
    if (i % 20 === 0) {
      CACHE.set(obj.id, obj);
      kept++;
    }
    if (CACHE.size > 1000) {
      const oldest = CACHE.keys().next().value;
      CACHE.delete(oldest);
    }
  }
  return kept;
}
let total = 0;
for (let n = 0; n < 40; n++) {
  total += work();
}
console.log(total, CACHE.size);
