// issue #397 回归：严格模式 arguments 三项语义与 delete 操作符收口。
// (c) delete arguments：函数环境绑定 deletable=false（§10.2.11 步骤 27/34），
//     DeleteBinding 返回 false（§9.1.1.1.8）。
// (b) strict delete 不可配置属性抛 TypeError（§13.5.5.9 步骤 5.d），sloppy
//     返回 false；proxy falsish trap 与基元接收者同规则。
// (a) strict 代码中 arguments 赋值是 early error，经 eval 迟到运行期。

// (c) sloppy delete arguments：直接与经箭头函数捕获两种形态。
function plain() {
  return delete arguments;
}
console.log(plain(1, 2));
function viaArrow() {
  var probe = () => delete arguments;
  return probe();
}
console.log(viaArrow(1));

// (b) sloppy delete：不可配置属性返回 false，缺失属性返回 true。
var target = {};
Object.defineProperty(target, "fixed", { value: 1, configurable: false });
console.log(delete target.fixed);
console.log(delete target.missing);

// (b) strict delete 不可配置属性抛 TypeError，可配置属性正常返回 true。
(function () {
  "use strict";
  try {
    delete target.fixed;
    console.log("unreachable");
  } catch (error) {
    console.log(error instanceof TypeError, error.message);
  }
  var configurable = { gone: 1 };
  console.log(delete configurable.gone);
})();

// (b) proxy deleteProperty trap 返回 falsish：sloppy false，strict 专属
// TypeError；Reflect.deleteProperty 只返回布尔，不受 strict 影响。
var vetoed = new Proxy({}, { deleteProperty() { return false; } });
console.log(delete vetoed.entry);
(function () {
  "use strict";
  try {
    delete vetoed.entry;
    console.log("unreachable");
  } catch (error) {
    console.log(error instanceof TypeError, error.message);
  }
  console.log(Reflect.deleteProperty(vetoed, "entry"));
})();

// (b) 基元接收者：字符串 length 与在界索引不可配置，其余键恒可删。
console.log(delete "abc".length, delete "abc"[1], delete "abc"[9], delete (5).x);
(function () {
  "use strict";
  try {
    delete "abc".length;
    console.log("unreachable");
  } catch (error) {
    console.log(error instanceof TypeError, error.message);
  }
})();

// (a) strict 代码中 arguments 赋值：eval 源在运行期抛 SyntaxError（可捕获）。
(function () {
  "use strict";
  try {
    eval("arguments = 10;");
    console.log("unreachable");
  } catch (error) {
    console.log(error instanceof SyntaxError, error.name);
  }
})();
