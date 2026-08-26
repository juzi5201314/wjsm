// sso: 创建、比较并索引零到六码元 ASCII 字符串
const WINDOW_MS = Number(process.env.BENCH_WINDOW_MS || 1000);
const WARMUP_MS = Number(process.env.BENCH_WARMUP_MS || 500);
const BASE = 'abcdef';
const EXPECTED = ['', 'a', 'ab', 'abc', 'abcd', 'abcde', 'abcdef'];

function work() {
  let total = 0;
  for (let length = 0; length <= 6; length++) {
    const value = BASE.slice(0, length);
    if (value === EXPECTED[length]) total += value.length;
    if (value.length > 0) total += value.at(-1).charCodeAt(0);
  }
  return total;
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
