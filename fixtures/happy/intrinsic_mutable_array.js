// FIX-02 回归：Array intrinsic 快路径必须遵守可变属性。
// 覆盖 Array.isArray（静态成员）与 Array.prototype.map（原型方法）的
// 赋值替换、删除、恢复，以及 spread / 可选调用形态。

// 1. 替换 Array.isArray：直接调用与 spread 调用都必须执行替换函数。
const originalIsArray = Array.isArray;
Array.isArray = (value) => "IS:" + typeof value;
console.log(Array.isArray([1, 2]));
console.log(Array.isArray(...["x"]));

// 2. 恢复后快路径行为回归原生。
Array.isArray = originalIsArray;
console.log(Array.isArray([1, 2]), Array.isArray("x"));

// 3. 删除 Array.isArray：调用抛 TypeError；可选调用短路且不求值实参。
delete Array.isArray;
try {
  Array.isArray([1]);
} catch (e) {
  console.log(e instanceof TypeError, typeof Array.isArray);
}
let argEvaluated = false;
console.log(Array.isArray?.((argEvaluated = true, [])), argEvaluated);
Array.isArray = originalIsArray;

// 4. 替换 Array.prototype.map：数组实例方法调用必须执行替换函数。
const originalMap = Array.prototype.map;
Array.prototype.map = function (fn) {
  return "MAP:" + this.length + ":" + fn(this[0], 0, this);
};
console.log([10, 20, 30].map((x) => x * 2));

// 5. 恢复后回归原生（含回调、索引参数）。
Array.prototype.map = originalMap;
console.log([1, 2, 3].map((x, i) => x + i).join(","));

// 6. 删除 Array.prototype.map：调用抛 TypeError。
delete Array.prototype.map;
try {
  [1].map((x) => x);
} catch (e) {
  console.log(e instanceof TypeError, [].map === undefined);
}
Array.prototype.map = originalMap;
console.log([4, 5].map((x) => x * x).join("|"));
