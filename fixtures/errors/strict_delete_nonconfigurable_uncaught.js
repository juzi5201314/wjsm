"use strict";
// strict delete 不可配置属性未捕获：TypeError 终止执行（§13.5.5.9 步骤 5.d）。
var obj = {};
Object.defineProperty(obj, "0", { configurable: false, value: 1 });
console.log("before");
delete obj[0];
console.log("unreachable");
