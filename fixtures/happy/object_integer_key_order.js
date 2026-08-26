// ECMA OrdinaryOwnPropertyKeys：integer-index 键升序在前，其余保插入序。
const mixed = { b: 1, 3: 2, a: 3 };
console.log("mixed keys:", JSON.stringify(Object.keys(mixed)));

const p = { 3: "a" };
p[5] = "b";
p.foo = 1;
console.log("p keys:", JSON.stringify(Object.keys(p)));
console.log("p json:", JSON.stringify(p));

let forIn = [];
for (const key in mixed) {
  forIn.push(key);
}
console.log("for-in:", JSON.stringify(forIn));
