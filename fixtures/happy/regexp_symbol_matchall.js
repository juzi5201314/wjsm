// RegExp.prototype[@@matchAll]（§22.2.6.14）与 String.prototype.matchAll
// （§22.1.3.14）：@@matchAll 是 RegExp 实例可见的合成方法，String.matchAll
// 按 GetMethod(regexp, @@matchAll) 转调；迭代器实例接线
// %RegExpStringIteratorPrototype%（§22.2.9.1），推进按 §22.2.9.2.1。

// --- 方法形态 ---
console.log(typeof /./[Symbol.matchAll], /./[Symbol.matchAll].name, /./[Symbol.matchAll].length);
console.log(/./[Symbol.matchAll] === /x/g[Symbol.matchAll]);

// --- 非 global：产出首个匹配后 [[Done]] 置真（步骤 11.b），且忽略 lastIndex ---
const one = /\w/[Symbol.matchAll]("a*b");
const first = one.next();
console.log(first.value[0], first.value.index, first.done, one.next().done);

// sticky 非 global：从 lastIndex 快照起精确匹配一次，原对象 lastIndex 不回写。
const stickyRe = /\w/y;
stickyRe.lastIndex = 2;
const stickyIt = stickyRe[Symbol.matchAll]("a*b");
console.log(stickyIt.next().value[0], stickyIt.next().done, stickyRe.lastIndex);

// --- global：完整迭代与捕获组 ---
const globalIt = /o(k)?/g[Symbol.matchAll]("bok ox");
console.log([...globalIt].map(m => m[0] + ":" + m.index + ":" + m[1]).join(","));

// global 空匹配按 AdvanceStringIndex 前进并在越过串尾后耗尽。
console.log([..."ab".matchAll(/(?:)/g)].map(m => m.index).join(","));

// --- String.prototype.matchAll 的 RegExpCreate 回退（步骤 3-5）---
console.log(JSON.stringify([..."abc".matchAll()].map(m => m[0])));
console.log([..."xnullz".matchAll(null)].map(m => m.index).join(","));

// 非 global RegExp 实参先抛（步骤 2.b，在取 @@matchAll 之前）。
try {
  "a".matchAll(/a/y);
} catch (error) {
  console.log(error.constructor.name + ": " + error.message);
}

// @@matchAll 的 brand 检查（V8 口径 incompatible receiver 文案）。
try {
  /a/g[Symbol.matchAll].call(null, "a");
} catch (error) {
  console.log(error.constructor.name + ": " + error.message);
}

// 用户自定义 @@matchAll 经 GetMethod 转调（this 为 pattern，实参为 receiver 串）。
const custom = {
  [Symbol.matchAll](s) {
    return "got:" + s + ":" + (this === custom);
  },
};
console.log("xy".matchAll(custom));

// --- 迭代器实例的原型与 helper 继承 ---
console.log(Object.getPrototypeOf(/a/g[Symbol.matchAll]("a")) === Object.getPrototypeOf("a".matchAll(/a/g)));
console.log("aXbXc".matchAll(/X/g).map(m => m.index).toArray().join(","));

// global 的 lastIndex 快照：迭代从快照起步，原对象 lastIndex 不回写。
const reused = /a/g;
reused.lastIndex = 2;
console.log([...reused[Symbol.matchAll]("ababa")].map(m => m.index).join(","), reused.lastIndex);
