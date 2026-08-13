// string-ops: 拼接 + slice + split + 模板插值的字符串负载
const WINDOW_MS = Number(process.env.BENCH_WINDOW_MS || 1000);
const WARMUP_MS = Number(process.env.BENCH_WARMUP_MS || 500);

const BASE = 'the quick brown fox jumps over the lazy dog';
const PARTS = 'alpha,beta,gamma,delta,epsilon,zeta,eta,theta,iota,kappa';

function work() {
  let s = '';
  for (let i = 0; i < 100; i++) {
    s += BASE + i;
  }
  const sliced = s.slice(10, 500);
  const parts = PARTS.split(',');
  const tpl = `${parts[0]}-${parts[parts.length - 1]}-${sliced.length}`;
  return tpl.length + sliced.length;
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
