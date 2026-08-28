"use strict";
// strict 模式下 [[Set]] 失败按 PutValue 步骤 6.c 抛 TypeError（消息与 V8
// 一致）：冻结对象 / getter-only / 不可写数据属性（自有与原型链）/ 不可
// 扩展对象 / sealed / symbol 键。数组 / proxy / callable / Reflect / IC
// 见 strict_setprop_typeerror_receivers.js；基元接收者见
// primitive_setprop_strict.js。
function show(tag, fn) {
  try {
    fn();
    console.log(tag, "NO-THROW");
  } catch (error) {
    console.log(tag, error.name, "|", error.message);
  }
}

const frozen = Object.freeze({ x: 1 });
show("frozen-prop", () => { frozen.x = 2; });
show("frozen-compound", () => { frozen.x += 5; });
show("frozen-increment", () => { frozen.x++; });
const dyn = "x";
show("frozen-computed", () => { frozen[dyn] = 3; });
show("frozen-destructure", () => { ({ a: frozen.x } = { a: 9 }); });
show("frozen-arr-destructure", () => { [frozen.x] = [9]; });
console.log("frozen-unchanged", frozen.x);

const getterOnly = {};
Object.defineProperty(getterOnly, "g", { get() { return 1; } });
show("getter-only", () => { getterOnly.g = 2; });

const nonWritable = {};
Object.defineProperty(nonWritable, "w", { value: 1, writable: false });
show("non-writable", () => { nonWritable.w = 2; });
console.log("non-writable-unchanged", nonWritable.w);

const nonExtensible = Object.preventExtensions({});
show("non-extensible", () => { nonExtensible.nu = 1; });
console.log("non-extensible-has", "nu" in nonExtensible);

const proto = {};
Object.defineProperty(proto, "p", { value: 1, writable: false });
const child = Object.create(proto);
show("inherited-non-writable", () => { child.p = 2; });

const protoGetter = {};
Object.defineProperty(protoGetter, "pg", { get() { return 1; } });
const child2 = Object.create(protoGetter);
show("inherited-getter-only", () => { child2.pg = 2; });

// sealed：既有属性仍可写，新增被拒。
const sealed = Object.seal({ s: 1 });
show("sealed-write", () => { sealed.s = 2; });
console.log("sealed-value", sealed.s);
show("sealed-add", () => { sealed.t = 1; });

// symbol 键同口径。
const sym = Symbol("k");
const symObj = {};
Object.defineProperty(symObj, sym, { get() { return 1; } });
show("symbol-getter-only", () => { symObj[sym] = 2; });
const symFrozen = Object.freeze({ [sym]: 1 });
show("symbol-frozen", () => { symFrozen[sym] = 2; });
