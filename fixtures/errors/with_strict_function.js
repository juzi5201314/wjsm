// 函数级 "use strict" 指令与类体均为严格代码：with 是 early error，
// 即使函数从未被调用也在编译期拒绝。
function f() {
  "use strict";
  with ({}) { }
}
console.log("unreachable");
