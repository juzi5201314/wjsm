// 栈溢出错误应显示 JS 函数名和源码位置（issue #64）
// 非尾位置递归（`+ 1` 在调用之后求值），避免被 tail_self_loop 改写成循环。
function recurse(n) {
  return recurse(n + 1) + 1;
}
recurse(0);
