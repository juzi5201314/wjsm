// RegExp 可作 [[Prototype]]（proto 槽 RegExp 标记位）：Object.create /
// setPrototypeOf / 字面量 __proto__ / __proto__ setter 全部接受；继承读取
// 归约自有 lastIndex 与方法后沿 %RegExp.prototype% 上行，访问器族对非
// RegExp receiver 作 brand 检查（§22.2.6），与 Node v22 逐字节一致。

const re = /ab/g;
const o1 = Object.create(re);
console.log("A", Object.getPrototypeOf(o1) === re);
const o2 = {};
Object.setPrototypeOf(o2, re);
console.log("B", Object.getPrototypeOf(o2) === re);
const o3 = { __proto__: re };
console.log("C", Object.getPrototypeOf(o3) === re);
const o4 = {};
o4.__proto__ = re;
console.log("D", Object.getPrototypeOf(o4) === re);

// 继承读取：自有 lastIndex 数据属性、%RegExp.prototype% 方法沿链可见。
console.log("E", o1.lastIndex, "lastIndex" in o1, typeof o1.exec);

// 访问器族（source / flags / 标志位）对非 RegExp receiver 抛 TypeError；
// generic flags getter 的首个内部读是 hasIndices（与 V8 报错口径一致）。
try { o1.source; } catch (e) { console.log("F", e.constructor.name, e.message); }
try { o1.flags; } catch (e) { console.log("G", e.constructor.name, e.message); }

// OrdinarySet 归约：链上 RegExp 的 lastIndex 是可写数据属性，写入在
// receiver 上建自有属性，不改 RegExp 本体。
const w = Object.create(re);
w.lastIndex = 5;
console.log("H", w.lastIndex, re.lastIndex, w.hasOwnProperty("lastIndex"));

// __lookupGetter__ 链行走命中 RegExp 自有数据属性层即终止返回 undefined。
console.log("I", o1.__lookupGetter__("lastIndex"), typeof RegExp.prototype.__lookupGetter__ === "function");

// receiver 自身是 RegExp 时访问器按 this=receiver 正常求值（brand 通过）。
console.log("J", /cd/i.source, /cd/i.flags, re.source, re.flags);

// HasProperty 沿链：lastIndex / 方法 / 访问器名对 in 可见，未知名缺失。
console.log("K", "exec" in o1, "source" in o1, "nope" in o1);
