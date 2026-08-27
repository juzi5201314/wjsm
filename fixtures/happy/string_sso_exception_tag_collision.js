// inline SSO 字符串的 7-bit/8-bit 码元载荷可能让 NaN-box bits 32–36 恰好等于
// TAG_EXCEPTION（0x5）：ASCII 第 5 个码元落在 0x50–0x5F（如 ']'），或 Latin-1
// 第 5 个码元低 5 位为 0b00101（如 '¥'）。后端 is_exception 判定必须并入 SSO
// marker 位排除这类字符串，否则会把它们误判为异常哨兵（InternalInvariant）。
const direct = JSON.stringify([2, 3]);
console.log(direct, direct.length);
console.log(JSON.stringify([2, 3]));

function returnsCollision() {
  return "[2,3]";
}
console.log(returnsCollision());

function passThrough(x) {
  return x;
}
// 第 5 个码元覆盖 'P'–'_'（bits 4–6 = 0b101）的代表性取样
for (const s of ["abcd]", "abcdP", "abcd_", "aaaaZ", "abcdP@", "[2,3]"]) {
  console.log(passThrough(s), s.length);
}
// Latin-1 SSO：第 5 个码元低 5 位为 0b00101。只断言长度与相等性，
// 不直接打印非 ASCII 码元（console 编码行为与本回归无关）。
const latin = "aaaa\u00a5";
console.log(passThrough(latin) === "aaaa\u00a5", latin.length);
console.log("done");
