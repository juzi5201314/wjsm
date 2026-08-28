// Object / Reflect 静态方法对 nullish 与非对象参数的 TypeError 矩阵：
// 文案对齐 V8（Node v22 逐字节一致），全部可被 catch 且不残留宿主 pending 异常。
function probe(label, fn) {
  try {
    const result = fn();
    console.log(label, "=>", typeof result === "object" && result !== null ? JSON.stringify(result) : String(result));
  } catch (error) {
    console.log(label, "!!", error.name + ": " + error.message);
  }
}

console.log("=== ToObject nullish TypeError ===");
probe("keys(undefined)", () => Object.keys(undefined));
probe("keys(null)", () => Object.keys(null));
probe("values(null)", () => Object.values(null));
probe("entries(undefined)", () => Object.entries(undefined));
probe("getOwnPropertyNames(null)", () => Object.getOwnPropertyNames(null));
probe("getOwnPropertySymbols(undefined)", () => Object.getOwnPropertySymbols(undefined));
probe("assign(null, {})", () => Object.assign(null, {}));
probe("getPrototypeOf(undefined)", () => Object.getPrototypeOf(undefined));
probe("getOwnPropertyDescriptor(null)", () => Object.getOwnPropertyDescriptor(null, "x"));
probe("getOwnPropertyDescriptors(undefined)", () => Object.getOwnPropertyDescriptors(undefined));
probe("defineProperties({}, null)", () => Object.defineProperties({}, null));
probe("create(null, null)", () => Object.create(null, null));

console.log("=== setPrototypeOf ===");
probe("setPrototypeOf(undefined, null)", () => Object.setPrototypeOf(undefined, null));
probe("setPrototypeOf(null, 5)", () => Object.setPrototypeOf(null, 5));
probe("setPrototypeOf({}, 5)", () => Object.setPrototypeOf({}, 5));
probe("setPrototypeOf(1, 5)", () => Object.setPrototypeOf(1, 5));

console.log("=== defineProperty / defineProperties ===");
probe("defineProperty(1)", () => Object.defineProperty(1, "x", {}));
probe("defineProperty(null)", () => Object.defineProperty(null, "x", {}));
probe("defineProperty({}, 'x', 1)", () => Object.defineProperty({}, "x", 1));
probe("defineProperty({}, 'x', null)", () => Object.defineProperty({}, "x", null));
probe("defineProperties(true, {})", () => Object.defineProperties(true, {}));
probe("defineProperties({}, {x: 's'})", () => Object.defineProperties({}, { x: "s" }));
probe("defineProperties({}, 1)", () => Object.defineProperties({}, 1));

console.log("=== create ===");
probe("create(undefined)", () => Object.create(undefined));
probe("create(1)", () => Object.create(1));
probe("create(true)", () => Object.create(true));
probe("create(Symbol(s))", () => Object.create(Symbol("s")));
probe("create('ab')", () => Object.create("ab"));

console.log("=== fromEntries ===");
probe("fromEntries(undefined)", () => Object.fromEntries(undefined));
probe("fromEntries(null)", () => Object.fromEntries(null));
probe("fromEntries(1)", () => Object.fromEntries(1));
probe("fromEntries(true)", () => Object.fromEntries(true));
probe("fromEntries({})", () => Object.fromEntries({}));
probe("fromEntries([1])", () => Object.fromEntries([1]));
probe("fromEntries('ab')", () => Object.fromEntries("ab"));
function* closable() {
  try {
    yield 1;
  } finally {
    console.log("iterator closed");
  }
}
probe("fromEntries(generator)", () => Object.fromEntries(closable()));

console.log("=== Reflect called on non-object ===");
probe("Reflect.ownKeys(undefined)", () => Reflect.ownKeys(undefined));
probe("Reflect.ownKeys(1)", () => Reflect.ownKeys(1));
probe("Reflect.getPrototypeOf(1)", () => Reflect.getPrototypeOf(1));
probe("Reflect.setPrototypeOf(1, null)", () => Reflect.setPrototypeOf(1, null));
probe("Reflect.setPrototypeOf({}, 1)", () => Reflect.setPrototypeOf({}, 1));
probe("Reflect.getOwnPropertyDescriptor(1)", () => Reflect.getOwnPropertyDescriptor(1, "x"));
probe("Reflect.defineProperty(1)", () => Reflect.defineProperty(1, "x", {}));
probe("Reflect.isExtensible(1)", () => Reflect.isExtensible(1));
probe("Reflect.preventExtensions(1)", () => Reflect.preventExtensions(1));

console.log("=== Reflect.setPrototypeOf false vs throw ===");
probe("frozen target", () => Reflect.setPrototypeOf(Object.freeze({}), { a: 1 }));
probe("cyclic proto", () => {
  const base = {};
  const derived = Object.create(base);
  return Reflect.setPrototypeOf(base, derived);
});

console.log("still alive");
