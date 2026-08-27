// 未捕获的对象 spread 源异常必须终止执行并以运行时错误退出，
// 而不是被吞掉后继续执行产生空对象。
function boom() {
  throw new Error("object spread boom");
}
const o = { ...boom() };
console.log("unreachable", Object.keys(o).length);
