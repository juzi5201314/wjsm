// 严格模式代码中 arguments 作为赋值目标是 early error（§13.1.3
// AssignmentTargetType 非 simple），即使函数从未被调用也在编译期拒绝。
function f() {
  "use strict";
  arguments = 7;
}
console.log("unreachable");
