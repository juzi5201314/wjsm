"use strict";
// strict 模式下对基元字符串写属性未捕获时必须以 TypeError 终止执行，
// 而不是宿主 InternalInvariant 崩溃或静默继续。
var s = "hello";
s.x = 1;
console.log("unreachable");
