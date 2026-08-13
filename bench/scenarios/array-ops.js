// array-ops: 高阶函数链 + 排序的组合数组负载
const WINDOW_MS = Number(process.env.BENCH_WINDOW_MS || 1000);
const WARMUP_MS = Number(process.env.BENCH_WARMUP_MS || 500);

const ARR = Array.from({ length: 1000 }, (_, i) => (i * 7919) % 10007);

function work() {
  const mapped = ARR.map((x) => x * 2).filter((x) => x % 3).reduce((a, b) => a + b, 0);
  const sorted = ARR.slice().sort((a, b) => b - a);
  return mapped + sorted[0] + sorted[sorted.length - 1];
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
