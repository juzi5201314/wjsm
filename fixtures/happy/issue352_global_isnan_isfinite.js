// 全局 isNaN / isFinite：先 ToNumber 再判断（ECMA-262 §19.2.3 / §19.2.2），
// 区别于 Number.isNaN / Number.isFinite 的无强转语义。
console.log(isNaN(NaN));
console.log(isNaN(1));
console.log(isNaN("x"));
console.log(isNaN("3"));
console.log(isNaN(undefined));
console.log(isNaN(null));
console.log(isNaN(true));
console.log(isNaN({ valueOf() { return NaN; } }));
console.log(isFinite(3));
console.log(isFinite("3"));
console.log(isFinite(Infinity));
console.log(isFinite(-Infinity));
console.log(isFinite(NaN));
console.log(isFinite("x"));
console.log(isFinite(undefined));
console.log(isFinite(null));
console.log(isFinite({ valueOf() { return 5; } }));
console.log(Number.isNaN("x"));
console.log(Number.isFinite("3"));
console.log(typeof isNaN);
console.log(typeof isFinite);
console.log(globalThis.isNaN === isNaN);
console.log(globalThis.isFinite === isFinite);
try {
  isNaN(Symbol());
} catch (e) {
  console.log(e.message);
}
