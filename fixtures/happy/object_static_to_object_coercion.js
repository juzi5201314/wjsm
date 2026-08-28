// Object 静态方法族的 ToObject / RequireObjectCoercible 收口矩阵：
// 基元（number/boolean/symbol/string）与 RegExp 走包装对象/奇异对象语义，
// 输出与 Node v22 逐字节一致。
console.log("=== enumerate on primitives ===");
console.log(JSON.stringify(Object.keys(1)), JSON.stringify(Object.keys(true)), JSON.stringify(Object.keys(Symbol("s"))));
console.log(JSON.stringify(Object.values(1)), JSON.stringify(Object.entries(true)));
console.log(JSON.stringify(Object.keys("ab")), JSON.stringify(Object.values("ab")), JSON.stringify(Object.entries("ab")));
console.log(JSON.stringify(Object.getOwnPropertyNames(1)), JSON.stringify(Object.getOwnPropertyNames("ab")));
console.log(JSON.stringify(Object.getOwnPropertySymbols(1)), JSON.stringify(Object.getOwnPropertySymbols("ab")));

console.log("=== enumerate on regexp ===");
console.log(JSON.stringify(Object.keys(/x/)), JSON.stringify(Object.getOwnPropertyNames(/x/)));
console.log(JSON.stringify(Object.getOwnPropertyDescriptor(/x/, "lastIndex")));
console.log(Reflect.ownKeys(/x/).join(","));

console.log("=== getPrototypeOf on primitives ===");
console.log(Object.getPrototypeOf(1) === Number.prototype);
console.log(Object.getPrototypeOf("a") === String.prototype);
console.log(Object.getPrototypeOf(true) === Boolean.prototype);
console.log(Object.getPrototypeOf(Symbol("x")) === Symbol.prototype);

console.log("=== setPrototypeOf primitive passthrough ===");
console.log(Object.setPrototypeOf(1, null), Object.setPrototypeOf("ab", null), Object.setPrototypeOf(true, null));

console.log("=== assign ToObject(target) ===");
const wrapped = Object.assign(1, { a: 2 });
console.log(typeof wrapped, wrapped instanceof Number, +wrapped, wrapped.a);
console.log(JSON.stringify(Object.assign({}, "ab", { x: 9 })));
console.log(JSON.stringify(Object.assign({}, 1, true, Symbol("s"))));

console.log("=== getOwnPropertyDescriptor(s) on primitives ===");
console.log(String(Object.getOwnPropertyDescriptor(1, "x")), String(Object.getOwnPropertyDescriptor(true, "toString")));
console.log(JSON.stringify(Object.getOwnPropertyDescriptor("ab", 0)));
console.log(JSON.stringify(Object.getOwnPropertyDescriptor("ab", "1")));
console.log(JSON.stringify(Object.getOwnPropertyDescriptor("ab", "length")));
console.log(String(Object.getOwnPropertyDescriptor("ab", 2)), String(Object.getOwnPropertyDescriptor("ab", "01")));
console.log(JSON.stringify(Object.getOwnPropertyDescriptors(1)));
console.log(JSON.stringify(Object.getOwnPropertyDescriptors("ab")));

console.log("=== proxy enumerate filters symbols ===");
const proxied = new Proxy({ [Symbol("q")]: 1, b: 2 }, {});
console.log(Object.getOwnPropertyNames(proxied).join(","), Object.keys(proxied).join(","), JSON.stringify(Object.entries(proxied)));
console.log(Reflect.ownKeys(proxied).map(String).join(","));

console.log("=== Reflect.ownKeys keeps symbols ===");
console.log(Reflect.ownKeys({ b: 2, [Symbol("a")]: 1 }).map(String).join(","));
