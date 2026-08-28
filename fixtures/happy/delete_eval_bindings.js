// eval 代码的 DeleteBinding：§19.2.1.3 EvalDeclarationInstantiation 用
// CreateMutableBinding(N, true) / CreateGlobalVarBinding(N, true) 创建
// eval 顶层 var 与函数声明（可删除，delete 返回 true）；let/const/class
// 走 eval 专属词法环境（deletable=false）。经桥接解析到调用方作用域的
// 声明式绑定不可删除；不可解析名返回 true。

// 间接 eval（全局 eval）：自建 var/函数可删，词法不可删。
(0, eval)(
  "var iv = 1; console.log('indirect var:', delete iv);" +
    "let il = 2; console.log('indirect let:', delete il);" +
    "function ifn() {} console.log('indirect fn:', delete ifn);" +
    "class IC {} console.log('indirect class:', delete IC);",
);
console.log("indirect undeclared:", (0, eval)("delete notDeclaredHere"));

// 直接 eval：调用方 var/let/const 经桥接解析，均不可删除且值完好。
function outer() {
  var ov = 1;
  let ol = 2;
  const oc = 3;
  eval(
    "console.log('direct outer:', delete ov, delete ol, delete oc)",
  );
  console.log("outer intact:", ov, ol, oc);
  // eval 自建 var 可删（返回值裁决）。
  eval("var dv = 5; console.log('direct eval var:', delete dv)");
  // eval 内不可解析名恒 true。
  eval("console.log('direct undeclared:', delete missingName)");
  // 调用方 arguments 绑定经桥接不可删除。
  eval("console.log('direct arguments:', delete arguments)");
}
outer();

// 嵌套函数内经桥接删除 eval 顶层不可解析名。
function outer2() {
  var keep = 1;
  eval("(function () { console.log('nested bridge:', delete keep, delete alsoMissing); })()");
  console.log("keep intact:", keep);
}
outer2();

// 受限全局名经 eval 同样不可删除。
console.log("eval restricted:", (0, eval)("delete undefined"), (0, eval)("delete NaN"));
