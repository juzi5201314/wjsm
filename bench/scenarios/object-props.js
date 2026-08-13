// object-props: 类构造 + getter + 方法的对象负载
const WINDOW_MS = Number(process.env.BENCH_WINDOW_MS || 1000);
const WARMUP_MS = Number(process.env.BENCH_WARMUP_MS || 500);

class Point {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }

  get norm() {
    return Math.hypot(this.x, this.y);
  }

  scale(factor) {
    return new Point(this.x * factor, this.y * factor);
  }
}

const POINTS = Array.from({ length: 100 }, (_, i) => new Point(i, i + 1));

function work() {
  let total = 0;
  for (let i = 0; i < POINTS.length; i++) {
    const p = POINTS[i];
    total += p.norm;
    const scaled = p.scale(0.5);
    total += scaled.x + scaled.y;
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
