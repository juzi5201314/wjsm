// inline SSO 字符串同样带 BOX_BASE，其 7-bit ASCII 码元载荷可让 NaN-box 的
// tag 位（32–36）恰好等于 TAG_OBJECT(0x8) 或 TAG_ARRAY(0xB)：第 5 码元 < 0x10
// 且第 6 码元 ≡ 1 (mod 4) → object；第 5 码元 '0'–'?' 且第 6 码元 ≡ 1 (mod 4)
// → array（如 "abcd3a"）。属性读 / 元素读的后端快路径在解句柄前必须并入 SSO
// marker 位（48–50）排除这类字符串，否则会把它们当成对象 / 数组句柄去解一个
// 越界句柄索引（崩溃或读到垃圾）。这里只走读路径断言 JS 语义正确。
function readMissing(o) {
  return o.missing;
}
function readLength(o) {
  return o.length;
}

// tag 位 == TAG_OBJECT(0x8)：第 5 码元 0x01（控制符），第 6 码元 'a'。
const objTag = String.fromCharCode(97, 98, 99, 100, 1, 97);
console.log(readLength(objTag), objTag.charCodeAt(4), readMissing(objTag));
console.log(typeof objTag.slice, objTag[0], objTag[5]);

// tag 位 == TAG_ARRAY(0xB)："abcd3a" 全可打印。
const arrTag = "abcd3a";
console.log(arrTag, readLength(arrTag), readMissing(arrTag));
console.log(arrTag[0], arrTag[4], arrTag.charCodeAt(4));

// object-tag 取样：第 5 码元遍历 0x00–0x0F，逐一确认属性读不误判、不崩溃。
let objOk = true;
for (let c = 0; c < 16; c++) {
  const s = String.fromCharCode(97, 98, 99, 100, c, 97);
  if (readLength(s) !== 6 || readMissing(s) !== undefined || s.charCodeAt(4) !== c) {
    objOk = false;
  }
}
// array-tag 取样：第 5 码元覆盖 '0'–'?'（0x30–0x3F）与第 6 码元 ≡ 1 (mod 4)。
let arrOk = true;
for (const b of ["abcd0a", "abcd3a", "abcd?a", "abcd;a", "abcd7a"]) {
  const s = b;
  if (readLength(s) !== 6 || readMissing(s) !== undefined || s[0] !== "a" || s[4] !== b[4]) {
    arrOk = false;
  }
}
console.log(objOk, arrOk);
console.log("done");
