// ArrayBuffer / SharedArrayBuffer 构造器的 ToIndex（§7.1.22）完整语义：
// length 与 options.maxByteLength 都执行 ToIntegerOrInfinity 的用户转换
// （@@toPrimitive / valueOf / toString，maxByteLength 经 Get 触发 getter），
// Symbol / BigInt 按 V8 文案 TypeError，越界按 V8 文案 RangeError，合法但
// 不可分配的长度按 "Array buffer allocation failed" 可恢复 RangeError
// （绝不触发宿主 allocator abort）。stdout 与 Node v22 逐字节一致。
function t(label, fn) {
  try {
    console.log(label, "ok", String(fn()));
  } catch (e) {
    console.log(label, e.constructor.name + "|" + e.message);
  }
}

// ── ArrayBuffer 构造器：length 的 ToIndex ──
t("ab(undefined)", () => new ArrayBuffer(undefined).byteLength);
t("ab(NaN)", () => new ArrayBuffer(NaN).byteLength);
t("ab(null)", () => new ArrayBuffer(null).byteLength);
t("ab(true)", () => new ArrayBuffer(true).byteLength);
t('ab("8")', () => new ArrayBuffer("8").byteLength);
t("ab(4.5)", () => new ArrayBuffer(4.5).byteLength);
t("ab(-0.9)", () => new ArrayBuffer(-0.9).byteLength);
t("ab(valueOf 4)", () => new ArrayBuffer({ valueOf() { return 4; } }).byteLength);
t("ab(Symbol)", () => new ArrayBuffer(Symbol()));
t("ab(10n)", () => new ArrayBuffer(10n));
t("ab(-1)", () => new ArrayBuffer(-1));
t("ab(2^53)", () => new ArrayBuffer(9007199254740992));
t("ab(Infinity)", () => new ArrayBuffer(Infinity));
t("ab(2^53-1)", () => new ArrayBuffer(9007199254740991));
t("ab(2^52)", () => new ArrayBuffer(2 ** 52));
// length 的 ToIndex 失败先于 options.maxByteLength 的 Get（getter 不执行）。
t("ab(Symbol,{get max})", () => new ArrayBuffer(Symbol(), { get maxByteLength() { console.log("max-read"); return 8; } }));

// ── ArrayBuffer 构造器：options.maxByteLength ──
t("ab max valueOf", () => new ArrayBuffer(2, { maxByteLength: { valueOf() { return 8; } } }).maxByteLength);
t('ab max "8"', () => new ArrayBuffer(2, { maxByteLength: "8" }).maxByteLength);
t("ab max getter", () => new ArrayBuffer(2, { get maxByteLength() { return 8; } }).maxByteLength);
t("ab options num", () => new ArrayBuffer(4, 5).resizable);
t("ab options null", () => new ArrayBuffer(4, null).resizable);
t("ab max NaN", () => new ArrayBuffer(2, { maxByteLength: NaN }));
t("ab max -1", () => new ArrayBuffer(2, { maxByteLength: -1 }));
t("ab max 2^53", () => new ArrayBuffer(0, { maxByteLength: 9007199254740992 }));
t("ab max 2^52", () => new ArrayBuffer(0, { maxByteLength: 2 ** 52 }));
t("ab max Symbol", () => new ArrayBuffer(0, { maxByteLength: Symbol() }));
t("ab max 8n", () => new ArrayBuffer(0, { maxByteLength: 8n }));
t("ab max < len", () => new ArrayBuffer(8, { maxByteLength: 4 }));

// ── SharedArrayBuffer 构造器 ──
t("sab(undefined)", () => new SharedArrayBuffer(undefined).byteLength);
t("sab(valueOf 4)", () => new SharedArrayBuffer({ valueOf() { return 4; } }).byteLength);
t("sab(Symbol)", () => new SharedArrayBuffer(Symbol()));
t("sab(-1)", () => new SharedArrayBuffer(-1));
t("sab(2^53)", () => new SharedArrayBuffer(9007199254740992));
t("sab(2^52)", () => new SharedArrayBuffer(2 ** 52));
t("sab max valueOf", () => new SharedArrayBuffer(2, { maxByteLength: { valueOf() { return 8; } } }).maxByteLength);
t("sab max 2^53", () => new SharedArrayBuffer(0, { maxByteLength: 9007199254740992 }));
t("sab max 2^52", () => new SharedArrayBuffer(0, { maxByteLength: 2 ** 52 }));
