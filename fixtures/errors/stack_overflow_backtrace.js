// 栈溢出错误应显示 JS 函数名和源码位置（issue #64）
function recurse(n) {
  return recurse(n + 1);
}
recurse(0);
