// FIX-02 回归：String intrinsic 快路径必须遵守可变属性。
// 覆盖 String.raw（静态成员）与 String.prototype.slice / trim（原型方法）的
// 赋值替换、删除、getter 替换、恢复，以及 spread / 可选调用形态。

// 1. 替换 String.raw：直接调用与 spread 调用都必须执行替换函数。
const originalRaw = String.raw;
String.raw = (parts, ...subs) => "RAW:" + subs.join("+");
console.log(String.raw({ raw: ["a", "b", "c"] }, 1, 2));
console.log(String.raw(...[{ raw: ["x", "y"] }, 7]));

// 2. 恢复后快路径行为回归原生。
String.raw = originalRaw;
console.log(String.raw({ raw: ["a", "b", "c"] }, 1, 2));

// 3. 删除 String.raw：调用必须抛 TypeError。
delete String.raw;
try {
  String.raw({ raw: ["q"] });
} catch (e) {
  console.log(e instanceof TypeError, typeof String.raw);
}
String.raw = originalRaw;

// 4. getter 替换：Object.defineProperty 安装的 getter 必须被调用。
let getterHits = 0;
Object.defineProperty(String, "raw", {
  configurable: true,
  get() {
    getterHits += 1;
    return () => "GETTER";
  },
});
console.log(String.raw({ raw: ["z"] }), getterHits);
Object.defineProperty(String, "raw", {
  configurable: true,
  writable: true,
  value: originalRaw,
});
console.log(String.raw({ raw: ["m", "n"] }, 5));

// 5. 替换 String.prototype.slice：字符串实例方法调用必须执行替换函数。
const originalSlice = String.prototype.slice;
String.prototype.slice = function (start) {
  return "SLICE:" + this.length + ":" + start;
};
console.log("hello".slice(1));
console.log("hello world".slice(0, 5));

// 6. 恢复后回归原生。
String.prototype.slice = originalSlice;
console.log("hello".slice(1));

// 7. 删除 String.prototype.slice：调用必须抛 TypeError。
delete String.prototype.slice;
try {
  "hello".slice(1);
} catch (e) {
  console.log(e instanceof TypeError, "hello".slice === undefined);
}
String.prototype.slice = originalSlice;
console.log("restored".slice(2));

// 8. 可选调用短路：删除后可选调用返回 undefined 且不求值实参。
delete String.prototype.trim;
let argEvaluated = false;
const trimmed = "  pad  ".trim?.((argEvaluated = true, 0));
console.log(trimmed, argEvaluated);
