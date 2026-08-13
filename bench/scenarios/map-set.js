// map-set: 插入/查找/删除各 100 次的 Map 与 Set 负载（大小保持稳定）
const WINDOW_MS = Number(process.env.BENCH_WINDOW_MS || 1000);
const WARMUP_MS = Number(process.env.BENCH_WARMUP_MS || 500);

const MAP = new Map();
const SET = new Set();
for (let i = 0; i < 100; i++) {
  MAP.set(i, i * i);
  SET.add(i);
}

function work() {
  let checks = 0;
  for (let i = 0; i < 100; i++) {
    MAP.set(i, i * i);
    MAP.get(i);
    MAP.delete(i);
    SET.add(i);
    SET.has(i);
    SET.delete(i);
  }
  return checks;
}

for (const end = performance.now() + WARMUP_MS; performance.now() < end;) work();

const ITERATIONS = Number(process.env.BENCH_ITERATIONS || 0);
if (ITERATIONS > 0) {
  const t0 = performance.now();
  for (let i = 0; i < ITERATIONS; i++) work();
  console.log(`ns_per_op=${((performance.now() - t0) * 1e6 / Math.max(ITERATIONS, 1)).toFixed(1)} iterations=${ITERATIONS}`);
} else {
  let iterations = 0;
  const t0 = performance.now();
  while (performance.now() - t0 < WINDOW_MS) { work(); iterations++; }
  console.log(`ns_per_op=${((performance.now() - t0) * 1e6 / Math.max(iterations, 1)).toFixed(1)} iterations=${iterations}`);
}
