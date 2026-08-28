"use strict";
// strict 写失败未捕获：TypeError 终止执行（PutValue 步骤 6.c）。
const frozen = Object.freeze({ x: 1 });
console.log("before");
frozen.x = 2;
console.log("unreachable");
