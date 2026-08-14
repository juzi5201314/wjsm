// math-binary-dispatch: 实参未证明 f64 的二元 Math 链，保留 call_dispatcher 基线。
const WINDOW_MS = Number(process.env.BENCH_WINDOW_MS || 1000);
const WARMUP_MS = Number(process.env.BENCH_WARMUP_MS || 500);

const N = 100_000;

function work() {
  let s = 0.0;
  const x = Number(process.env.MATH_X || "1.0001");
  const y = Number(process.env.MATH_Y || "2.0002");
  for (let i = 0; i < N; i++) {
    const a = x + i * 0.0001;
    const b = y - i * 0.0001;
    s += Math.atan2(a, b) + Math.pow(a, b);
  }
  return s;
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
