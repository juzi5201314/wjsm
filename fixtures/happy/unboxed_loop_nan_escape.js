// 浮点寄存器里的 NaN 是硬件默认 QNaN，其位模式与 NaN-Box 前缀相同。
// 逃逸出循环时必须换成规范 NaN，否则会被运行时误判成句柄。
function makeNan() {
  let a = Infinity;
  let b = Infinity;
  let r = 0;
  for (let i = 0; i < 1; i++) {
    r = a - b;
  }
  return r;
}

function divideNan() {
  let z = 0;
  let r = 1;
  for (let i = 0; i < 1; i++) {
    r = z / z;
  }
  return r;
}

function accumulateNan(n) {
  let total = 0;
  for (let i = 0; i < n; i++) {
    total += Math.sqrt(-1);
  }
  return total;
}

function report(v) {
  console.log(typeof v, v, Number.isNaN(v), v === v, String(v), JSON.stringify(v));
}

report(makeNan());
report(divideNan());
report(accumulateNan(3));

// NaN 穿过对象属性、数组元素与 Map 键后仍必须是 number。
const box = { value: makeNan() };
console.log(typeof box.value, Number.isNaN(box.value));

const list = [];
list.push(makeNan());
console.log(list.length, Number.isNaN(list[0]), list.join("|"));

const seen = new Map();
seen.set(makeNan(), "nan");
console.log(seen.get(NaN));
console.log(seen.size);

// 无穷大不是 NaN，必须原样穿过逃逸点。
function makeInfinity() {
  let r = 0;
  for (let i = 0; i < 1; i++) {
    r = 1 / 0;
  }
  return r;
}
console.log(makeInfinity(), -makeInfinity(), Number.isFinite(makeInfinity()));

// NaN 参与比较与算术时不得因规范化而改变语义。
function nanCompare() {
  let n = makeNan();
  let hits = 0;
  for (let i = 0; i < 3; i++) {
    if (n < i || n >= i || n === n) {
      hits++;
    }
  }
  return hits;
}
console.log(nanCompare());
console.log(Number.isNaN(makeNan() + 1), Number.isNaN(makeNan() * 0));
