// closures: 闭包创建/调用与计数器自增负载
const WINDOW_MS = Number(process.env.BENCH_WINDOW_MS || 1000);
const WARMUP_MS = Number(process.env.BENCH_WARMUP_MS || 500);

function counter() {
  let value = 0;
  return function increment() {
    value += 1;
    return value;
  };
}

function work() {
  let total = 0;
  const add = (a) => (b) => a + b;
  const inc = counter();
  for (let i = 0; i < 1000; i++) {
    total += add(1)(2);
    inc();
  }
  return total + inc();
}

for (const end = performance.now() + WARMUP_MS; performance.now() < end;) work();

let iterations = 0;
const t0 = performance.now();
while (performance.now() - t0 < WINDOW_MS) { work(); iterations++; }
console.log(`ns_per_op=${((performance.now() - t0) * 1e6 / Math.max(iterations, 1)).toFixed(1)} iterations=${iterations}`);
