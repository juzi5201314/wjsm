// tlab-object: 反复创建小对象并读取 name/value/length 字段
const WINDOW_MS = Number(process.env.BENCH_WINDOW_MS || 1000);
const WARMUP_MS = Number(process.env.BENCH_WARMUP_MS || 500);
const OBJECTS_PER_OPERATION = 64;

function work() {
  let total = 0;
  for (let i = 0; i < OBJECTS_PER_OPERATION; i++) {
    const object = { name: i, value: i * 2, length: i + 1 };
    total += object.name + object.value + object.length;
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
