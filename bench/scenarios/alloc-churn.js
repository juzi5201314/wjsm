// alloc-churn: 临时对象分配风暴，约 5% 进入有界 Map（超 1000 条淘汰最旧）
const WINDOW_MS = Number(process.env.BENCH_WINDOW_MS || 1000);
const WARMUP_MS = Number(process.env.BENCH_WARMUP_MS || 500);

const CACHE = new Map();
let nextId = 0;

function work() {
  let kept = 0;
  for (let i = 0; i < 100; i++) {
    const obj = { id: nextId++, payload: `p${i}`, values: [i, i * 2, i * 3] };
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
