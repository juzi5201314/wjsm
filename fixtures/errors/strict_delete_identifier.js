"use strict";
// 严格模式代码中 delete 作用于 IdentifierReference 是编译期 SyntaxError
// （§13.5.1.1），对所有绑定统一成立，不看绑定是否存在或可删。
var x = 1;
delete x;
console.log("unreachable");
