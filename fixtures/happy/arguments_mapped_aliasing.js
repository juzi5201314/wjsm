// mapped arguments 的 [[ParameterMap]] 双向绑定（ES §10.4.4）：
// sloppy 简单参数列表下形参与 arguments 索引属性互为别名；
// 严格模式无映射；delete / defineProperty 降级 / freeze 解除映射后各自独立。

// 写形参 → arguments 可见；写 arguments → 形参可见；严格模式互不影响。
function w1(a) { a = 2; return arguments[0]; }
function w2(a) { arguments[0] = 2; return a; }
function s1(a) { "use strict"; a = 2; return arguments[0]; }
function s2(a) { "use strict"; arguments[0] = 2; return a; }
console.log("basic", w1(1), w2(1), s1(1), s2(1));

// delete 解除映射：属性删除后绑定保留删除前快照，双方独立演化。
function d1(a) { delete arguments[0]; a = 5; return arguments[0]; }
function d2(a) { delete arguments[0]; arguments[0] = 9; return a; }
console.log("delete", d1(1), d2(1));

// defineProperty：数据降级 writable:false 先写值再解除；accessor 降级保留 define 前绑定值。
function dp1(a) {
  Object.defineProperty(arguments, "0", { value: 7, writable: false });
  a = 8;
  return [a, arguments[0]].join(",");
}
function dp2(a) {
  Object.defineProperty(arguments, "0", { get() { return 42; } });
  return [a, arguments[0]].join(",");
}
console.log("defineProperty", dp1(1), dp2(1));

// freeze 全部解除（绑定快照冻结时值）；seal 只收紧 configurable，映射存续。
function fz(a, b) {
  arguments[0] = 3;
  Object.freeze(arguments);
  a = 10;
  arguments[1] = 11;
  return [a, arguments[0], b, arguments[1]].join(",");
}
function sl(a) { Object.seal(arguments); a = 4; return arguments[0]; }
console.log("freeze-seal", fz(1, 2), sl(1));

// 闭包捕获：嵌套箭头/函数读写别名形参仍与 arguments 同步。
function c1(a) { const set = (v) => { a = v; }; set(5); return arguments[0]; }
function c2(a) { function inner() { return a; } arguments[0] = 6; return inner(); }
console.log("closure", c1(1), c2(1));

// 复合赋值 / 逻辑复合 / update 表达式全部写穿映射。
function u1(a) { a++; return arguments[0]; }
function u2(a) { arguments[0] = 10; return a += 5; }
function u3(a) { a ||= 9; return arguments[0]; }
function u4(a) { a &&= 9; return arguments[0]; }
console.log("compound", u1(1), u2(1), u3(0), u4(0));

// generator / async body：wrapper 物化的同一对象经续体槽传入，别名跨 yield/await 存续。
function* g1(a) { a = 4; yield arguments[0]; arguments[0] = 5; yield a; }
const it = g1(1);
console.log("generator", it.next().value, it.next().value);
async function a1(x) { x = 8; await Promise.resolve(); return arguments[0]; }
a1(1).then((v) => console.log("async", v));

// 重复形参：仅最后一次出现入 map（sloppy 后者胜）。
function dup(a, a) { a = 99; return [arguments[0], arguments[1]].join(","); }
console.log("dup", dup(1, 2));

// 实参不足：index >= argc 从创建起就不在 map 中，形参独立演化。
function m1(a, b) { b = 3; return [arguments.length, arguments[1], b].join(","); }
console.log("missing", m1(1));

// 解构赋值 / for-of 头 / with 回退写入均收口到映射。
function ds(a) { [a] = [4]; return arguments[0]; }
function fo(a) { for (a of [9]) {} return arguments[0]; }
function wt(a) { arguments; with ({}) { a = 3; } return arguments[0]; }
function ws(a) { arguments; var o = { a: 0 }; with (o) { a = 3; } return [arguments[0], o.a].join(","); }
console.log("targets", ds(1), fo(1), wt(1), ws(1));

// 重赋 arguments 标识符不影响别名基座；spread 读取属性即绑定真值。
function r1(a) { var args = arguments; arguments = null; a = 5; return args[0]; }
function sp(a, b) { a = 10; return [...arguments].join(","); }
console.log("misc", r1(1), sp(1, 2));
