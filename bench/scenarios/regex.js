// regex: 三种常用正则的 test + exec 混合负载
const WINDOW_MS = Number(process.env.BENCH_WINDOW_MS || 1000);
const WARMUP_MS = Number(process.env.BENCH_WARMUP_MS || 500);

const EMAIL_RE = /^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$/;
const HEX_RE = /^#?([0-9a-fA-F]{6}|[0-9a-fA-F]{3})$/;
const ISO_DATE_RE = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d{3}))?(?:Z|[+-]\d{2}:\d{2})$/;

const EMAIL = 'user.name+tag@example-domain.co.uk';
const HEX = '#a1B2c3';
const ISO_DATE = '2026-07-31T14:30:00.123Z';

function work() {
  let checks = 0;
  if (EMAIL_RE.test(EMAIL)) checks++;
  if (HEX_RE.test(HEX)) checks++;
  if (ISO_DATE_RE.test(ISO_DATE)) checks++;
  const emailMatch = EMAIL_RE.exec(EMAIL);
  const hexMatch = HEX_RE.exec(HEX);
  const dateMatch = ISO_DATE_RE.exec(ISO_DATE);
  return checks + (emailMatch ? emailMatch.length : 0) + (hexMatch ? hexMatch.length : 0) + (dateMatch ? dateMatch.length : 0);
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
