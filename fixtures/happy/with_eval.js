// with 与 direct eval 互操作：eval 内 with、with 内 eval、绑定回写、完成值。
const o = { x: 5 };
console.log(eval("with(o){x}"));
console.log(eval("with(o){ x = 6; x * 2 }"));
console.log(o.x);

with (o) {
  console.log(eval("x"));
  eval("x = 7");
}
console.log(o.x);

function f() {
  const inner = { v: 1 };
  with (inner) {
    return eval("v + 100");
  }
}
console.log(f());

// 完成值：空体、条件、循环、label break、try/finally。
console.log(eval("with(o){ 42 }"));
console.log(eval("with(o){ }"));
console.log(eval("1; with(o){ }"));
console.log(eval("with(o){ if (x > 5) { 'big' } }"));
console.log(eval("with(o){ while (false) { 1 } }"));
console.log(eval("label: with(o){ break label; 'skipped' }"));
console.log(eval("try { with(o){ 'in-try' } } finally { 'in-finally' }"));

// eval 代码里的自由名经调用方 with 链解析（含遮蔽判定）。
let shadow = "static";
with ({ shadow: "object" }) {
  console.log(eval("shadow"));
  eval("shadow = 'eval-written'");
}
console.log(shadow);

// 严格 eval 内 with 是 SyntaxError（可捕获），调用方严格性亦继承。
try {
  eval('"use strict"; with(o){}');
} catch (e) {
  console.log(e instanceof SyntaxError);
}
