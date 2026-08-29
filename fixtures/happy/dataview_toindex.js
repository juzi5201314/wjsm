// DataView 构造器与 get / set 方法的 ToIndex（§7.1.22）完整语义：
// byteOffset / byteLength / requestIndex 与 set 的 value 都执行用户转换
// （@@toPrimitive / valueOf / toString），Symbol / BigInt 按 V8 文案
// TypeError，越界按各站点 V8 文案 RangeError。检查顺序以 Node v22 实测
// 为准：转换副作用先行，detach TypeError 与边界 RangeError 在后（构造器
// offset 的数值越界 RangeError 先于 detach 检查）。stdout 逐字节一致。
function t(label, fn) {
  try {
    console.log(label, "ok", String(fn()));
  } catch (e) {
    console.log(label, e.constructor.name + "|" + e.message);
  }
}

// ── DataView 构造器：byteOffset / byteLength 的 ToIndex ──
t("dv offset valueOf", () => new DataView(new ArrayBuffer(8), { valueOf() { return 2; } }).byteOffset);
t("dv offset undefined", () => new DataView(new ArrayBuffer(8), undefined).byteOffset);
t("dv offset Symbol", () => new DataView(new ArrayBuffer(8), Symbol()));
t("dv offset 1n", () => new DataView(new ArrayBuffer(8), 1n));
t("dv offset -1", () => new DataView(new ArrayBuffer(8), -1));
t("dv offset 2^53", () => new DataView(new ArrayBuffer(8), 9007199254740992));
t("dv offset 9", () => new DataView(new ArrayBuffer(8), 9));
t("dv len valueOf", () => new DataView(new ArrayBuffer(8), 0, { valueOf() { return 3; } }).byteLength);
t("dv len Symbol", () => new DataView(new ArrayBuffer(8), 0, Symbol()));
t("dv len -1", () => new DataView(new ArrayBuffer(8), 0, -1));
t("dv len 2^53", () => new DataView(new ArrayBuffer(8), 0, 9007199254740992));
t("dv len 100", () => new DataView(new ArrayBuffer(8), 0, 100));
// detach 检查后于 offset 的 ToIndex（副作用先行）、先于 offset 边界检查。
const detachedAb = new ArrayBuffer(8);
detachedAb.transfer();
t("dv detached side-effect", () => new DataView(detachedAb, { valueOf() { console.log("dv-ctor-offset"); return 0; } }));
t("dv detached -1", () => new DataView(detachedAb, -1));
t("dv detached 16", () => new DataView(detachedAb, 16));
// offset / byteLength 的 valueOf 中途 detach → TypeError。
t("dv mid-detach offset", () => { const ab = new ArrayBuffer(8); return new DataView(ab, { valueOf() { ab.transfer(); return 0; } }); });
t("dv mid-detach len", () => { const ab = new ArrayBuffer(8); return new DataView(ab, 0, { valueOf() { ab.transfer(); return 2; } }); });

// ── DataView get / set：requestIndex 与 value 的转换 ──
t("dv get idx valueOf", () => { const dv = new DataView(new ArrayBuffer(8)); dv.setUint8(1, 9); return dv.getUint8({ valueOf() { return 1; } }); });
t("dv set idx/val valueOf", () => { const dv = new DataView(new ArrayBuffer(8)); dv.setUint8({ valueOf() { return 1; } }, { valueOf() { return 7; } }); return dv.getUint8(1); });
t("dv get idx NaN", () => new DataView(new ArrayBuffer(8)).getUint8(NaN));
t("dv get idx undefined", () => new DataView(new ArrayBuffer(8)).getUint8(undefined));
t("dv get idx 1.5", () => { const dv = new DataView(new ArrayBuffer(8)); dv.setUint8(1, 9); return dv.getUint8(1.5); });
t("dv get idx Symbol", () => new DataView(new ArrayBuffer(8)).getUint8(Symbol()));
t("dv get idx 1n", () => new DataView(new ArrayBuffer(8)).getUint8(1n));
t("dv set idx Symbol", () => new DataView(new ArrayBuffer(8)).setUint8(Symbol(), 1));
t("dv set val Symbol", () => new DataView(new ArrayBuffer(8)).setUint8(0, Symbol()));
t("dv set val 1n", () => new DataView(new ArrayBuffer(8)).setUint8(0, 1n));
t("dv get idx -1", () => new DataView(new ArrayBuffer(8)).getUint8(-1));
t("dv get idx 2^53", () => new DataView(new ArrayBuffer(8)).getUint8(9007199254740992));
t("dv get idx 2^52", () => new DataView(new ArrayBuffer(8)).getUint8(2 ** 52));
t("dv setUint32 idx -1", () => new DataView(new ArrayBuffer(8)).setUint32(-1, 0));
// getBig / setBig 的 requestIndex 同样过 ToIndex。
t("dv getBig idx valueOf", () => { const dv = new DataView(new ArrayBuffer(16)); dv.setBigInt64(8, 7n); return dv.getBigInt64({ valueOf() { return 8; } }); });
t("dv getBigU idx Symbol", () => new DataView(new ArrayBuffer(16)).getBigUint64(Symbol()));
t("dv setBig idx -1", () => new DataView(new ArrayBuffer(16)).setBigInt64(-1, 1n));
// detach 后：index / value 的转换副作用先行，随后 TypeError。
const detachedAb2 = new ArrayBuffer(8);
const detachedDv = new DataView(detachedAb2);
detachedAb2.transfer();
t("dv detached get side", () => detachedDv.getUint8({ valueOf() { console.log("dv-get-idx"); return 0; } }));
t("dv detached set side", () => detachedDv.setUint8({ valueOf() { console.log("dv-set-idx"); return 0; } }, { valueOf() { console.log("dv-set-val"); return 1; } }));
t("dv detached get -1", () => detachedDv.getUint8(-1));
// value 的 ToNumber 先于越界 RangeError。
t("dv set val-before-oob", () => new DataView(new ArrayBuffer(4)).setUint8(100, { valueOf() { console.log("dv-oob-val"); return 1; } }));
// index 的 valueOf 中途 detach → TypeError。
t("dv mid-detach get", () => { const ab = new ArrayBuffer(8); const dv = new DataView(ab); return dv.getUint8({ valueOf() { ab.transfer(); return 0; } }); });
