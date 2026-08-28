"use strict";
// strict 写失败 TypeError 的特殊接收者：数组（命名属性 / 字典下标）、
// proxy（trap falsish / 无 trap 冻结目标）、callable（不可写 name）、
// Reflect.set / Object.assign 口径、IC 训练后重复失败。普通对象见
// strict_setprop_typeerror.js。
function show(tag, fn) {
  try {
    fn();
    console.log(tag, "NO-THROW");
  } catch (error) {
    console.log(tag, error.name, "|", error.message);
  }
}

// 数组命名属性：不可写数据属性 / getter-only。
const arr = [];
Object.defineProperty(arr, "nw", { value: 1, writable: false });
show("array-non-writable", () => { arr.nw = 2; });
console.log("array-nw-unchanged", arr.nw);
Object.defineProperty(arr, "ag", { get() { return 1; } });
show("array-getter-only", () => { arr.ag = 2; });

// 数组字典下标：defineProperty 收紧 writable 后写入被拒且值不变。
const dictArr = [10, 20];
Object.defineProperty(dictArr, "0", { writable: false });
show("array-elem-non-writable", () => { dictArr[0] = 99; });
console.log("array-elem-unchanged", dictArr[0]);
const accArr = [1];
Object.defineProperty(accArr, "5", { get() { return 42; } });
show("array-elem-getter-only", () => { accArr[5] = 9; });

// proxy：set trap 返回 falsish / 无 trap 的冻结目标。
const proxyFalsish = new Proxy({}, { set() { return false; } });
show("proxy-falsish", () => { proxyFalsish.x = 1; });
const proxyFrozen = new Proxy(Object.freeze({ x: 1 }), {});
show("proxy-no-trap-frozen", () => { proxyFrozen.x = 2; });

// callable receiver：不可写自有数据属性（name）。V8 消息嵌入原始函数源码，
// wjsm 不保留源码文本（native-code 形态），故只断言消息主体前缀。
function fn() {}
try {
  fn.name = "q";
  console.log("fn-name", "NO-THROW");
} catch (error) {
  const prefix = "Cannot assign to read only property 'name' of function 'function fn() {";
  console.log("fn-name", error.name, "|", error.message.startsWith(prefix));
}
console.log("fn-name-unchanged", fn.name);

// Reflect.set 不抛，返回 false；Object.assign 按 Set(to, key, value, true) 抛。
const frozen = Object.freeze({ x: 1 });
console.log("reflect-frozen", Reflect.set(frozen, "x", 9));
console.log("reflect-proxy-falsish", Reflect.set(proxyFalsish, "x", 9));
show("object-assign", () => { Object.assign(Object.freeze({ q: 1 }), { q: 2 }); });

// 同一赋值点（IC 训练后）重复失败必须每次都抛且值不变。
function writeX(target, value) { target.x = value; }
const warm = { x: 0 };
for (let i = 0; i < 4; i++) writeX(warm, i);
let threw = 0;
const icFrozen = Object.freeze({ x: 100 });
for (let i = 0; i < 4; i++) {
  try { writeX(icFrozen, i); } catch (error) { threw++; }
}
console.log("ic-frozen", threw, icFrozen.x, warm.x);
