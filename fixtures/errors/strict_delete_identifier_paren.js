// 函数级 "use strict" 指令同样构成严格代码；§13.5.1.1 的括号规则递归
// 适用：delete (((x))) 与 delete x 同为编译期 SyntaxError。
function f() {
  "use strict";
  var x = 1;
  delete (((x)));
}
f();
console.log("unreachable");
