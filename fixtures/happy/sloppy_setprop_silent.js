// sloppy 模式下 [[Set]] 失败静默（PutValue 步骤 6.b）：不抛、不改值。
// strict 对照见 strict_setprop_typeerror.js。
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
console.log("frozen-unchanged", frozen.x);

const getterOnly = {};
Object.defineProperty(getterOnly, "g", { get() { return 1; } });
show("getter-only", () => { getterOnly.g = 2; });
console.log("getter-unchanged", getterOnly.g);

const nonWritable = {};
Object.defineProperty(nonWritable, "w", { value: 1, writable: false });
show("non-writable", () => { nonWritable.w = 2; });
console.log("non-writable-unchanged", nonWritable.w);

const nonExtensible = Object.preventExtensions({});
show("non-extensible", () => { nonExtensible.nu = 1; });
console.log("non-extensible-has", "nu" in nonExtensible);

// proxy set trap falsish：静默（此前误报内部错误）。
const proxyFalsish = new Proxy({}, { set() { return false; } });
show("proxy-falsish", () => { proxyFalsish.x = 1; });
console.log("proxy-x", proxyFalsish.x);

// 数组命名属性不可写：静默且不得改值（此前被无条件覆盖）。
const arr = [];
Object.defineProperty(arr, "nw", { value: 1, writable: false });
show("array-non-writable", () => { arr.nw = 2; });
console.log("array-nw-unchanged", arr.nw);

// 数组字典下标不可写：静默且值不变。
const dictArr = [10, 20];
Object.defineProperty(dictArr, "0", { writable: false });
show("array-elem-non-writable", () => { dictArr[0] = 99; });
console.log("array-elem-unchanged", dictArr[0]);

// 失败写入不得训练 IC：循环内反复失败后值仍不变。
function writeX(target, value) { target.x = value; }
const icFrozen = Object.freeze({ x: 100 });
for (let i = 0; i < 4; i++) writeX(icFrozen, i);
console.log("ic-frozen-unchanged", icFrozen.x);

// 解构赋值成员目标正常写入（此前被静默丢弃）。
const box = {};
({ a: box.v } = { a: 11 });
[box.w] = [22];
({ p: box.q = 42 } = {});
console.log("destructure-targets", box.v, box.w, box.q);
