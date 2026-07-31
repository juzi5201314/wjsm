// arithmetic: 浮点标量循环（不可向量化的跨迭代依赖）
const WINDOW_MS = Number(process.env.BENCH_WINDOW_MS || 1000);
const WARMUP_MS = Number(process.env.BENCH_WARMUP_MS || 500);

const N = 100_000;

function work() {
  let s = 0.0;
  for (let i = 0; i < N; i++) s += i * 1.0001;
  return s;
}

for (const end = performance.now() + WARMUP_MS; performance.now() < end;) work();

let iterations = 0;
const t0 = performance.now();
while (performance.now() - t0 < WINDOW_MS) { work(); iterations++; }
console.log(`ns_per_op=${((performance.now() - t0) * 1e6 / Math.max(iterations, 1)).toFixed(1)} iterations=${iterations}`);
