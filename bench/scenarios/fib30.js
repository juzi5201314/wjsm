// fib30: 尾递归不可优化的斐波那契，压函数调用栈
const WINDOW_MS = Number(process.env.BENCH_WINDOW_MS || 1000);
const WARMUP_MS = Number(process.env.BENCH_WARMUP_MS || 500);

function fib(n) {
  if (n < 2) return n;
  return fib(n - 1) + fib(n - 2);
}

function work() {
  fib(30);
}

for (const end = performance.now() + WARMUP_MS; performance.now() < end;) work();

let iterations = 0;
const t0 = performance.now();
while (performance.now() - t0 < WINDOW_MS) { work(); iterations++; }
console.log(`ns_per_op=${((performance.now() - t0) * 1e6 / Math.max(iterations, 1)).toFixed(1)} iterations=${iterations}`);
