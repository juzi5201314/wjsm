// math-unary-clif: 已内联为 CLIF 浮点 opcode 的 6 个 Math 一元函数，作为 typed thunk 上限参照。
const WINDOW_MS = Number(process.env.BENCH_WINDOW_MS || 1000);
const WARMUP_MS = Number(process.env.BENCH_WARMUP_MS || 500);

const N = 100_000;

function work() {
  let s = 0.0;
  const x = 1.0001;
  for (let i = 0; i < N; i++) {
    const t = x + i * 0.0001;
    s += Math.abs(t) + Math.sqrt(t) + Math.ceil(t) + Math.floor(t) + Math.trunc(t) + Math.fround(t);
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
