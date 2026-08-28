// 严格模式下对具名函数表达式自身名字赋值：CreateImmutableBinding(name, false)
// 的不可变绑定，严格写点抛运行时 TypeError（§9.1.1.1.5 步骤 5），未捕获则中止。
"use strict";
(function f() {
  f = 1;
})();
