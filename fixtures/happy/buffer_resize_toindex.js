// resize / transfer / grow / slice 的 ToIndex 与 ToIntegerOrInfinity 完整
// 语义：newLength / start / end 都执行用户转换（@@toPrimitive / valueOf /
// toString），Symbol / BigInt 按 V8 文案 TypeError。检查顺序以 Node v22
// 实测为准：转换副作用先行；resize 的 detached TypeError 先于数值越界
// RangeError，transfer 相反；transfer 的负值按 kInvalidArrayBufferLength、
// ≥2^53 按方法名 kInvalidLength 分叉；分配失败按 "Array buffer allocation
// failed" 可恢复 RangeError 且源保持原状。stdout 与 Node v22 逐字节一致。
function t(label, fn) {
  try {
    console.log(label, "ok", String(fn()));
  } catch (e) {
    console.log(label, e.constructor.name + "|" + e.message);
  }
}

// ── ArrayBuffer.prototype.resize ──
t("resize valueOf", () => { const ab = new ArrayBuffer(4, { maxByteLength: 8 }); ab.resize({ valueOf() { return 6; } }); return ab.byteLength; });
t("resize Symbol", () => new ArrayBuffer(4, { maxByteLength: 8 }).resize(Symbol()));
t("resize 6n", () => new ArrayBuffer(4, { maxByteLength: 8 }).resize(6n));
t("resize -1", () => new ArrayBuffer(4, { maxByteLength: 8 }).resize(-1));
t("resize 2^53", () => new ArrayBuffer(4, { maxByteLength: 8 }).resize(9007199254740992));
t("resize Infinity", () => new ArrayBuffer(4, { maxByteLength: 8 }).resize(Infinity));
t("resize 2^52", () => new ArrayBuffer(4, { maxByteLength: 8 }).resize(2 ** 52));
t("resize 9>max8", () => new ArrayBuffer(4, { maxByteLength: 8 }).resize(9));
t("resize NaN", () => { const ab = new ArrayBuffer(4, { maxByteLength: 8 }); ab.resize(NaN); return ab.byteLength; });
// detach 后：转换副作用先行，detached TypeError 先于数值越界 RangeError。
const detachedRab = new ArrayBuffer(4, { maxByteLength: 8 });
detachedRab.transfer();
t("resize detached side", () => detachedRab.resize({ valueOf() { console.log("resize-side"); return 0; } }));
t("resize detached -1", () => detachedRab.resize(-1));
t("resize mid-detach", () => { const ab = new ArrayBuffer(4, { maxByteLength: 8 }); ab.resize({ valueOf() { ab.transfer(); return 2; } }); });

// ── ArrayBuffer.prototype.transfer / transferToFixedLength ──
t("transfer valueOf", () => { const ab = new ArrayBuffer(4); const moved = ab.transfer({ valueOf() { return 6; } }); return moved.byteLength + "," + ab.detached; });
t("t2fl valueOf", () => new ArrayBuffer(4).transferToFixedLength({ valueOf() { return 2; } }).byteLength);
t("transfer Symbol", () => new ArrayBuffer(4).transfer(Symbol()));
t("transfer -1", () => new ArrayBuffer(4).transfer(-1));
t("transfer 2^53", () => new ArrayBuffer(4).transfer(9007199254740992));
t("transfer Infinity", () => new ArrayBuffer(4).transfer(Infinity));
t("t2fl 2^53", () => new ArrayBuffer(4).transferToFixedLength(9007199254740992));
t("t2fl -1", () => new ArrayBuffer(4).transferToFixedLength(-1));
// 分配失败：RangeError 且源保持原状（未 detach）。
const allocFailSrc = new ArrayBuffer(4);
t("transfer 2^52", () => allocFailSrc.transfer(2 ** 52));
t("transfer 2^52 source", () => allocFailSrc.byteLength + "," + allocFailSrc.detached);
t("transfer 17>max16", () => new ArrayBuffer(4, { maxByteLength: 16 }).transfer(17));
// detach 后：数值越界 RangeError 先于 detached TypeError。
const detachedTab = new ArrayBuffer(4);
detachedTab.transfer();
t("transfer detached -1", () => detachedTab.transfer(-1));
t("transfer detached 2^53", () => detachedTab.transfer(9007199254740992));
t("transfer detached 2^52", () => detachedTab.transfer(2 ** 52));
t("transfer mid-detach", () => { const ab = new ArrayBuffer(4, { maxByteLength: 8 }); return ab.transfer({ valueOf() { ab.transfer(); return 2; } }); });

// ── SharedArrayBuffer.prototype.grow ──
t("grow valueOf", () => { const sab = new SharedArrayBuffer(4, { maxByteLength: 16 }); sab.grow({ valueOf() { return 8; } }); return sab.byteLength; });
t("grow Symbol", () => new SharedArrayBuffer(4, { maxByteLength: 16 }).grow(Symbol()));
t("grow 8n", () => new SharedArrayBuffer(4, { maxByteLength: 16 }).grow(8n));
t("grow -1", () => new SharedArrayBuffer(4, { maxByteLength: 16 }).grow(-1));
t("grow 2^53", () => new SharedArrayBuffer(4, { maxByteLength: 16 }).grow(9007199254740992));
t("grow 2^52", () => new SharedArrayBuffer(4, { maxByteLength: 16 }).grow(2 ** 52));
t("grow NaN len0", () => { const sab = new SharedArrayBuffer(0, { maxByteLength: 8 }); sab.grow(NaN); return sab.byteLength; });
// 固定长度 SAB 的品牌检查先于 ToIndex（Symbol 也报 incompatible receiver）。
t("nogrow Symbol", () => new SharedArrayBuffer(4).grow(Symbol()));

// ── slice 的相对索引（ToIntegerOrInfinity）──
t("ab slice valueOf", () => new ArrayBuffer(8).slice({ valueOf() { return 2; } }).byteLength);
t("ab slice Symbol", () => new ArrayBuffer(8).slice(Symbol()));
t("ab slice 2n", () => new ArrayBuffer(8).slice(2n));
t("ab slice undef end", () => new ArrayBuffer(8).slice(0, undefined).byteLength);
t("ab slice -Infinity", () => new ArrayBuffer(8).slice(-Infinity, Infinity).byteLength);
t("ab slice mid-detach", () => { const ab = new ArrayBuffer(8); return ab.slice({ valueOf() { ab.transfer(); return 0; } }); });
t("sab slice valueOf", () => new SharedArrayBuffer(8).slice({ valueOf() { return 2; } }).byteLength);
t("sab slice Symbol", () => new SharedArrayBuffer(8).slice(Symbol()));
t("sab slice undef end", () => new SharedArrayBuffer(8).slice(0, undefined).byteLength);
