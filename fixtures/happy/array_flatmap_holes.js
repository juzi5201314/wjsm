// flatMap 的 FlattenIntoArray（§23.1.3.13.1）：内层洞经 HasProperty 跳过，
// 原型链继承索引可观察，字典数组洞位 accessor 经完整 [[Get]] 读出。
console.log("inner-hole:", JSON.stringify([1].flatMap(() => [, 2])));
console.log("outer-hole:", JSON.stringify([, 1].flatMap(x => [x])));
Array.prototype[0] = 7;
console.log("inherited-inner:", JSON.stringify([1].flatMap(() => [, 2])));
console.log("inherited-outer:", JSON.stringify([, 1].flatMap(x => [x, x])));
delete Array.prototype[0];
{
  const inner = [, 2];
  Object.defineProperty(inner, "0", { get() { return 5; } });
  console.log("inner-accessor:", JSON.stringify([1].flatMap(() => inner)));
}
console.log("non-array:", JSON.stringify([1, 2].flatMap(x => x * 10)));
