// GC + async/await 验收：在 await 前后分配对象，验证跨 GC 周期引用仍存活。
async function demo() {
  const before = { val: 1 };
  await Promise.resolve(undefined);
  const after = { val: 2 };
  // 触发多次分配/GC 周期，验证 before 没被回收
  for (let i = 0; i < 50000; i++) {
    const tmp = { x: i };
  }
  console.log(before.val, after.val);
}
demo();
