// 具名函数表达式自身名字绑定的核心 lowering 形态（IR 快照配套）：
// funcEnv 按每次求值新建、闭包创建后 InitializeBinding、体内自引用经
// 闭包捕获、非严格写静默忽略、严格写在写点抛运行时 TypeError。
var g = function g2(n) {
  g2 = 0;
  return n > 0 ? g2(n - 1) : n;
};
console.log(g(2));

var fns = [];
for (let i = 0; i < 2; i++) {
  fns.push(function named() {
    return [named, i];
  });
}
console.log(fns[0]()[0] === fns[0], fns[0]()[0] === fns[1]()[0], fns[1]()[1]);

var caught = (function f() {
  "use strict";
  try {
    f = 1;
  } catch (e) {
    return e instanceof TypeError;
  }
})();
console.log(caught);
