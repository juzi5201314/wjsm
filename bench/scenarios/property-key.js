// property-key: 反复读写 name/value/length 等短 ASCII 属性键
const WINDOW_MS = Number(process.env.BENCH_WINDOW_MS || 1000);
const WARMUP_MS = Number(process.env.BENCH_WARMUP_MS || 500);
const ACCESS_COUNT = 128;
const RECORD = { name: 0, value: 1, length: 2 };

function work() {
  let total = 0;
  for (let i = 0; i < ACCESS_COUNT; i++) {
    RECORD.name = RECORD.name + 1;
    RECORD.value = RECORD.name + RECORD.length;
    total += RECORD.name + RECORD.value + RECORD.length;
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
