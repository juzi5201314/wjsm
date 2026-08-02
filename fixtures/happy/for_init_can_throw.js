// for-init 为 can_throw 表达式（Bin 运算/调用/成员访问）时，init 的 store 与
// 异常分叉必须保留：init 变量应正确初始化，循环按条件执行（issue #346）。
let n = 0;
for (const end1 = 1 + 2; n < end1;) n++;
console.log("bin-init", n);

n = 0;
for (const end2 = Math.max(1, 3); n < end2;) n++;
console.log("call-init", n);

n = 0;
for (const end3 = [1, 2, 3, 4].length; n < end3;) n++;
console.log("member-init", n);

// 对照：纯 Const init 行为不变
n = 0;
for (const end4 = 3; n < end4;) n++;
console.log("const-init", n);

// for-init 表达式真实抛异常时应被 try/catch 捕获，而非流入循环
try {
  for (const end5 = 1n + 1; n < end5;) n++;
  console.log("bigint-mix no-throw");
} catch (e) {
  console.log("bigint-mix caught");
}
