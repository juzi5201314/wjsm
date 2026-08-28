// 严格模式 delete 标识符是 early error（§13.5.1.1）：eval / Function 源码
// 在编译期抛可捕获的 SyntaxError，宿主代码捕获后继续执行。

// eval 源自带指令：SyntaxError 可捕获，消息为 V8 同口径文本。
try {
  eval('"use strict"; delete x;');
  console.log("unreachable");
} catch (error) {
  console.log(error instanceof SyntaxError, error.name);
  console.log(
    error.message.includes("Delete of an unqualified identifier in strict mode."),
  );
}

// direct eval 继承调用方严格位：eval 源不含指令同样违例。
(function () {
  "use strict";
  try {
    eval("delete q;");
    console.log("unreachable");
  } catch (error) {
    console.log(error instanceof SyntaxError, error.name);
  }
})();

// 括号规则递归适用（§13.5.1.1 注）：delete (((x))) 同为 SyntaxError。
try {
  eval('"use strict"; delete (((x)));');
  console.log("unreachable");
} catch (error) {
  console.log(error instanceof SyntaxError, error.name);
}

// Function 构造器体自带指令：编译期 SyntaxError 同样可捕获。
try {
  new Function('"use strict"; delete x;');
  console.log("unreachable");
} catch (error) {
  console.log(error instanceof SyntaxError, error.name);
}

// 严格代码中合法的 delete 形式不受影响：成员删除正常返回 true。
(function () {
  "use strict";
  var target = { gone: 1 };
  console.log(delete target.gone, "gone" in target);
})();

// sloppy 间接 eval：eval 创建的 var 绑定 deletable=true，delete 返回 true
// （§19.2.1.1 步骤 12 → CreateMutableBinding(name, true)）。
console.log((0, eval)("var leaked = 1; delete leaked;"));
