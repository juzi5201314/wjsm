// 不逃逸 RECORD：循环内 name/value/length 读写应被标量替换，多次调用仍累积。
const RECORD = { name: 0, value: 1, length: 2 };
function work() {
  let total = 0;
  for (let i = 0; i < 3; i = i + 1) {
    RECORD.name = RECORD.name + 1;
    RECORD.value = RECORD.name + RECORD.length;
    total = total + RECORD.name + RECORD.value + RECORD.length;
  }
  return total;
}
console.log(work());
console.log(work());
