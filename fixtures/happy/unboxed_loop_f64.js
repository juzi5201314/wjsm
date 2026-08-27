// 纯数值归纳变量常驻浮点寄存器后，循环仍必须给出精确的 IEEE-754 结果。
function sumSquares(n) {
  let total = 0;
  for (let i = 0; i < n; i++) {
    total += i * i;
  }
  return total;
}

function harmonic(n) {
  let total = 0.0;
  for (let i = 1; i <= n; i++) {
    total += 1 / i;
  }
  return total;
}

function alternating(n) {
  let sign = 1;
  let total = 0;
  for (let i = 0; i < n; i++) {
    total += sign * (i + 0.5);
    sign = -sign;
  }
  return total;
}

// 循环一次都不执行时，归纳变量必须停在初值上。
function emptyLoop() {
  let i = 0;
  while (i < 0) {
    i = i + 1;
  }
  return i;
}

console.log(sumSquares(0));
console.log(sumSquares(1));
console.log(sumSquares(10));
console.log(harmonic(1));
console.log(harmonic(4));
console.log(alternating(0));
console.log(alternating(5));
console.log(emptyLoop());
console.log(typeof sumSquares(10), typeof harmonic(4), typeof emptyLoop());

// 负零必须保号：打标只规范化 NaN，不碰其他位模式。
function negativeZero() {
  let z = 0;
  for (let i = 0; i < 1; i++) {
    z = z * -1;
  }
  return z;
}
console.log(Object.is(negativeZero(), -0));
console.log(1 / negativeZero());

// 极小/极大值经浮点寄存器往返后不得丢精度。
function scale(x, times) {
  let v = x;
  for (let i = 0; i < times; i++) {
    v = v * 2;
  }
  return v;
}
// 次正规数经浮点寄存器往返后必须逐位不变（用等值判断避开数字格式化差异）。
console.log(scale(Number.MIN_VALUE, 3) === Number.MIN_VALUE * 8);
console.log(scale(Number.MAX_VALUE, 1));
console.log(scale(0.1, 1));
