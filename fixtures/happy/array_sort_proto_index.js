// sort / toSorted 的 SortIndexedProperties（§23.1.3.30.1）按
// HasProperty/Get 观察原型链继承索引：sort 跳洞收集含继承值，写回后成
// 自有元素；toSorted 读穿洞取继承值。
Array.prototype[0] = 7;
{
  const a = [, 1];
  a.sort();
  console.log("sort:", JSON.stringify(a), a.length, 0 in a);
}
console.log("toSorted:", JSON.stringify([, 1].toSorted()));
delete Array.prototype[0];
