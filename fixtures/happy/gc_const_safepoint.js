// BigInt/RegExp Const GC safepoint: BigIntFromLiteral/RegExpCreate 期间 live handles 保护。
// 创建对象填满 local handles，定义 BigInt 和 RegExp 常量（调用 host 函数分配堆对象），
// 验证 handles 在 GC 后仍存活。
function test() {
  let a = { x: 42 };
  let b = { y: 99 };

  // BigInt 常量: BigIntFromLiteral 分配 bigint_table 节点，可能触发 GC
  let big = 12345678901234567890n;

  // RegExp 常量: RegExpCreate 分配 regexp 堆对象，可能触发 GC
  let re = /hello/;

  // 大量分配进一步逼 GC
  let garbage = [];
  for (let i = 0; i < 200; i++) {
    garbage.push({ v: i });
  }

  // 验证 a, b 的 handles 在 GC 后仍正确
  return a.x + b.y;
}
console.log(test());
