// §9.1.1.1.8 DeleteBinding：声明式环境记录的绑定（var/let/const/形参/
// 函数名/类名/catch 参/arguments/具名函数表达式名）均以 deletable=false
// 创建，sloppy 下 delete 返回 false 且绑定完好；不可解析引用返回 true
// （§13.5.1.2 步骤 3.b）。

// 顶层各声明类别。
var vx = 1;
let ly = 2;
const cz = 3;
function gf() {}
class CC {}
console.log("var:", delete vx, vx);
console.log("let:", delete ly, ly);
console.log("const:", delete cz, cz);
console.log("funcdecl:", delete gf, typeof gf);
console.log("classdecl:", delete CC, typeof CC);

// 函数内：形参、局部 var/let/const、嵌套函数名、类名。
function inner(p) {
  var b = 1;
  let l = 2;
  const c = 3;
  function nested() {}
  class NC {}
  console.log("param:", delete p, p);
  console.log("locals:", delete b, delete l, delete c, delete nested, delete NC);
}
inner(0);

// catch 参数绑定不可删除。
try {
  throw 1;
} catch (e) {
  console.log("catch-param:", delete e, e);
}

// 隐式 arguments 绑定（§10.2.11 CreateMutableBinding("arguments", false)）。
(function () {
  console.log("arguments:", delete arguments, arguments.length);
})(7, 8);

// 具名函数表达式自身名字（CreateImmutableBinding，§9.1.1.1.8 步骤 3）。
(function nf() {
  console.log("fn-expr-name:", delete nf, typeof nf);
})();

// 闭包捕获的外层绑定同为声明式绑定。
function mk() {
  var captured = 1;
  return function () {
    console.log("captured:", delete captured, captured);
  };
}
mk()();

// TDZ 不影响可删性裁决：声明执行前 delete 返回 false 而非抛 ReferenceError。
console.log("tdz:", delete fwd);
let fwd = 9;

// 受限全局名（HasRestrictedGlobalProperty）恒不可删除。
console.log("restricted:", delete undefined, delete NaN, delete Infinity);

// 括号透传 Reference（§13.2.6.5）：delete (((x))) 与 delete x 同裁决。
(function () {
  var p = 1;
  console.log("paren:", delete (((p))), p);
})();

// 未声明名：不可解析引用，sloppy 恒 true。
console.log("undeclared:", delete neverDeclaredAnywhere);

// 块级函数声明（Annex B）为块作用域声明式绑定。
{
  function blockFn() {}
  console.log("block-fn:", delete blockFn, typeof blockFn);
}
