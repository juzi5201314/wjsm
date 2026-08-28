"use strict";
// 严格模式代码中 eval 作为赋值目标与 arguments 同规则（§13.1.3）：
// 编译期 SyntaxError。
eval = 1;
console.log("unreachable");
