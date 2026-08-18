// Unicode property escapes 系统覆盖（Unicode 17 / regress），需 /u。

// General Category（全名与别名）
console.log(/\p{Letter}/u.test("A"));
console.log(/\p{L}/u.test("A"));
console.log(/\p{Nd}/u.test("5"));
console.log(/\P{Letter}/u.test("123"));
console.log(/\P{Number}/u.test("a"));

// Binary properties
console.log(/\p{ASCII}/u.test("A"));
console.log(/\p{Emoji}/u.test("😀"));
console.log(/\p{Hex}/u.test("F"));
console.log(/\P{ASCII}/u.test("中"));

// Script / Script_Extensions（全名与别名）
console.log(/\p{Script=Latin}/u.test("café"));
console.log(/\p{sc=Latn}/u.test("a"));
console.log(/\p{Script_Extensions=Hani}/u.test("中"));
console.log(/\p{scx=Hani}/u.test("中"));
console.log(/\P{Script=Latin}/u.test("中"));

// Unicode 17 Script（与 Phase 1 manifest unicode 17.0.0 对齐）
console.log(/\p{Script=Todhri}/u.test(String.fromCodePoint(0x105C0)));
console.log(/\p{sc=Todr}/u.test(String.fromCodePoint(0x105C0)));

// 无 Unicode flag 时不是 property escape（Annex B identity）
console.log(/\p{Letter}/.test("p{Letter}"));
console.log(/\p{Letter}/.test("A"));

// 非法属性 / 错误 Unicode 模式 flag → SyntaxError（运行时构造）
var ok = false;
try {
  new RegExp("\\p{NotARealProperty}", "u");
} catch (e) {
  ok = e instanceof SyntaxError;
}
console.log(ok);

ok = false;
try {
  new RegExp("\\p{ASCII=Yes}", "u");
} catch (e) {
  ok = e instanceof SyntaxError;
}
console.log(ok);

ok = false;
try {
  // property of strings 需要 /v；仅 /u 为 SyntaxError
  new RegExp("\\p{RGI_Emoji}", "u");
} catch (e) {
  ok = e instanceof SyntaxError;
}
console.log(ok);

ok = false;
try {
  new RegExp("\\p{Script=FooBarBazInvalid}", "u");
} catch (e) {
  ok = e instanceof SyntaxError;
}
console.log(ok);
