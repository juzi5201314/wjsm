// G1 concurrent mark / cleanup 压力：先把对象晋升到 old，再断开唯一强引用。
// 显式 gc() 必须完成 mark cleanup；后续输出证明长循环仍可正常运行。

const holder = { old: { value: 41 }, churn: null };

for (let i = 0; i < 1200; i++) {
  holder.churn = { i, next: { j: i + 1 } };
}

gc();
gc();
console.log(holder.old.value);

holder.old = null;

for (let k = 0; k < 1200; k++) {
  holder.churn = { k, tail: [k, k + 1, k + 2] };
}

gc();
console.log("g1-mark-cleanup-ok");
