// LICM 泛化到 JS 函数：f 是纯调用（无参、无状态读写），
// 提升与否输出都应正确（s += f() 的 f 每次返回相同值 42）。
// 5 次迭代 s = 5 * 42 = 210。
function f() {
  return 42;
}
function g() {
  let i = 0;
  let s = 0;
  while (i < 5) {
    s += f();
    i++;
  }
  console.log(s);
}
g();
