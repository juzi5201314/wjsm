// 循环内复合赋值 + await：`s` 在 await 前读出的临时值跨 suspend 存活，
// 必须经 continuation 溢出/恢复（曾触发 "definition of %N does not dominate use"）。
async function total() {
  let s = 0;
  for (let i = 0; i < 3; i++) {
    s += await Promise.resolve(2);
  }
  return s;
}

total().then((v) => console.log(v));
