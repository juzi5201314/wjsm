// 未捕获的 ToPropertyKey 转换异常（对象键 toString 抛出）必须终止执行，
// 且属性不得以 "[object Object]" 键写入。
const o = {};
const key = {
  toString() {
    throw new Error("to property key boom");
  },
};
o[key] = 1;
console.log("unreachable", o);
