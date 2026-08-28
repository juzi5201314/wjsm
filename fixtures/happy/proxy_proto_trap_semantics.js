// Proxy [[GetPrototypeOf]] / [[Get]] / IsCallable 语义（§10.5.1、§10.5.8、
// §7.2.3）：trap 异常原样上抛、非法返回值抛 TypeError、无 trap 委托保留
// receiver、callable Proxy 被 typeof 与 Invoke 认可，与 Node v22 逐字节一致。

// trap 抛出的异常原样传播（§10.5.1 步骤 7 的 ? Call）。
const p1 = new Proxy({}, { getPrototypeOf() { throw new Error("boom"); } });
try { Object.getPrototypeOf(p1); } catch (e) { console.log("A", e.message); }

// 返回值既非 Object 也非 null 抛 TypeError（步骤 8）。
const p2 = new Proxy({}, { getPrototypeOf() { return 42; } });
try { Object.getPrototypeOf(p2); } catch (e) { console.log("B", e.constructor.name, e.message); }

// RegExp 是合法的 Object 返回值。
const re = /x/;
const p3 = new Proxy({}, { getPrototypeOf() { return re; } });
console.log("C", Object.getPrototypeOf(p3) === re);

// 无 get trap 时委托 target.[[Get]](P, Receiver)：链上 getter 的 this
// 保持原始 receiver（§10.5.8 步骤 6）。
const target = { get me() { return this === outer; } };
const inner = new Proxy(target, {});
const outer = Object.create(inner);
console.log("D", outer.me);

// callable Proxy：typeof 为 function，toLocaleString 的 Invoke 认可其可调用。
const cp = new Proxy(function () { return "via-proxy"; }, {});
console.log("E", typeof cp);
console.log("F", ({ toString: cp }).toLocaleString());

// isPrototypeOf 链行走经 [[GetPrototypeOf]]：链上 Proxy 的 trap 生效，
// 异常原样传播。
const base = {};
const proxyProto = new Proxy(Object.create(base), {});
const leaf = Object.create(proxyProto);
console.log("G", base.isPrototypeOf(leaf), Object.prototype.isPrototypeOf(leaf));
const throwing = new Proxy({}, { getPrototypeOf() { throw new Error("chain!"); } });
const leaf2 = Object.create(throwing);
try { base.isPrototypeOf(leaf2); } catch (e) { console.log("H", e.message); }

// HasProperty 链上 Proxy 走 has trap，trap 异常上抛（§10.5.7）。
const hp = new Proxy({}, { has(t, k) { if (k === "boom") throw new Error("has!"); return k === "yes"; } });
const child = Object.create(hp);
console.log("I", "yes" in child, "no" in child);
try { void ("boom" in child); } catch (e) { console.log("J", e.message); }
