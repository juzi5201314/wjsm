// %String.prototype% 固有属性表：方法为堆原型链上真实不可枚举自有数据属性
// （§22.1.3），取值后可经 call / apply / bind 复用；length / constructor /
// @@iterator 含内，包装对象沿包装原型链命中，输出与 Node v22 逐字节一致。
console.log(typeof "".match === "function");
const taken = "hello".match;
console.log(taken === String.prototype.match, taken === "world".match);
console.log(JSON.stringify(taken.call("hello", /l+/)[0]));
console.log(JSON.stringify(taken.apply("hello", [/l/g])));
console.log(JSON.stringify(String.prototype.match.bind("wjsm-2026")(/\d+/)[0]));
console.log(String.prototype.slice.call("abcdef", 1, 4));
console.log(String.prototype.slice.call(12345, 1));
console.log(String.prototype.slice.call(new String("abcdef"), 2, 4));
console.log(String.prototype.toUpperCase.call(new String("up")));
console.log("".match.name, "".match.length);

for (const name of [
  "at", "charAt", "concat", "endsWith", "includes", "indexOf", "localeCompare",
  "match", "matchAll", "normalize", "padStart", "repeat", "replace",
  "replaceAll", "search", "slice", "split", "substring", "startsWith",
  "toString", "trim", "trimStart", "trimEnd", "toLowerCase", "toUpperCase",
  "valueOf",
]) {
  const desc = Object.getOwnPropertyDescriptor(String.prototype, name);
  console.log(
    name, desc.writable, desc.enumerable, desc.configurable,
    typeof desc.value, desc.value.name, desc.value.length,
  );
}
const lengthDesc = Object.getOwnPropertyDescriptor(String.prototype, "length");
console.log(lengthDesc.value, lengthDesc.writable, lengthDesc.enumerable, lengthDesc.configurable);
const ctorDesc = Object.getOwnPropertyDescriptor(String.prototype, "constructor");
console.log(ctorDesc.value === String, ctorDesc.writable, ctorDesc.enumerable, ctorDesc.configurable);

console.log(Object.getPrototypeOf(String.prototype) === Object.prototype);
console.log(String.prototype.hasOwnProperty("match"), "match" in String.prototype);
console.log(JSON.stringify(Object.keys(String.prototype)));
console.log(JSON.stringify(String.prototype.toString()), JSON.stringify(String.prototype.valueOf()));
console.log(Object.prototype.toString.call(String.prototype));
console.log(String.prototype.length);

const wrapped = new String("hi");
console.log(Object.getPrototypeOf(wrapped) === String.prototype);
console.log(typeof wrapped.match, wrapped.slice(1), "match" in wrapped, wrapped.hasOwnProperty("match"));
console.log(wrapped.length, new String("abc")[1]);

console.log("abc"[Symbol.iterator] === String.prototype[Symbol.iterator]);
const iterDesc = Object.getOwnPropertyDescriptor(String.prototype, Symbol.iterator);
console.log(typeof iterDesc.value, iterDesc.writable, iterDesc.enumerable, iterDesc.configurable);
console.log(iterDesc.value.name, iterDesc.value.length);
console.log([..."ab"].join("|"));

const child = Object.create(String.prototype);
console.log(typeof child.slice, typeof child.match);

console.log(typeof "".hasOwnProperty, "".hasOwnProperty("length"));

String.prototype.match = function patched() { return "patched"; };
const replaced = "zzz".match;
console.log(typeof replaced, replaced.name, replaced.call("zzz"));
delete String.prototype.match;
console.log(typeof "".match, "match" in String.prototype);
