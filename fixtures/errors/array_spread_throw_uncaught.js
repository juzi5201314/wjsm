// 未捕获的数组 spread 源异常必须终止执行并以运行时错误退出，
// 而不是被吞掉后继续执行产生空数组。
function boom() {
  throw new Error("spread boom");
}
const a = [...boom()];
console.log("unreachable", a.length);
