// GetProp/SetProp/GetElem/SetElem GC safepoint: 属性访问期间 live handles spill 保护。
// 创建多个对象填满 local handles，执行属性访问（可能经 host import 触发 GC），
// 验证所有 handles 在 GC 后仍存活。
function test() {
  let a = { x: 10 };
  let b = { y: 20 };
  let obj = { val: 5, arr: [100, 200] };

  // GetProp + SetProp: obj.val 的读取和写入可能触发 GC
  obj.val = obj.val + 1;

  // GetElem + SetElem: obj.arr[0] 的元素读写可能触发 GC
  obj.arr[0] = obj.arr[0] + 1000;

  // 大量分配逼 GC 在属性访问期间触发
  let garbage = [];
  for (let i = 0; i < 200; i++) {
    garbage.push({ v: i });
  }

  // 验证 a, b, obj 的 handles 在 GC 后仍正确
  return a.x + b.y + obj.val + obj.arr[0];
}
console.log(test());
