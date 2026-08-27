// 身份逃逸的 RECORD 走 GetProp/SetProp IC；name/value/length 共用 trio mega-slot。
const RECORD = { name: 0, value: 1, length: 2 };
const leak = {};
leak.inner = RECORD;
function work() {
  RECORD.name = RECORD.name + 1;
  RECORD.value = RECORD.name + RECORD.length;
  return RECORD.name + RECORD.value + RECORD.length;
}
console.log(work());
console.log(work());
console.log(leak.inner.name, leak.inner.value, leak.inner.length);
