// 未捕获的对象字面量计算键异常必须终止执行并以运行时错误退出，
// 且属性值不得求值。
function boomKey() {
  throw new Error("computed key boom");
}
function valueMustNotRun() {
  console.log("value should not print");
  return 1;
}
const o = { [boomKey()]: valueMustNotRun() };
console.log("unreachable", o);
