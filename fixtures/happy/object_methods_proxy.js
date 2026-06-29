// Test Object.* methods on proxies
console.log("=== Object.keys on proxy ===");
const target1 = { a: 1, b: 2, c: 3 };
const handler1 = {
  ownKeys(target) {
    console.log("ownKeys trap");
    return Object.keys(target);
  },
  getOwnPropertyDescriptor(target, prop) {
    return Object.getOwnPropertyDescriptor(target, prop);
  }
};
const proxy1 = new Proxy(target1, handler1);
console.log("keys:", JSON.stringify(Object.keys(proxy1)));

console.log("\n=== Object.entries on proxy ===");
const target3 = { foo: "bar", baz: "qux" };
const handler3 = {};
const proxy3 = new Proxy(target3, handler3);
console.log("entries:", JSON.stringify(Object.entries(proxy3)));

console.log("\n=== Object.values on proxy ===");
const targetValues = { keep: 1, skip: 2 };
const handlerValues = {
  ownKeys(target) {
    console.log("values ownKeys trap");
    return ["keep"];
  },
  getOwnPropertyDescriptor(target, prop) {
    return { value: target[prop], enumerable: true, configurable: true };
  },
  get(target, prop, receiver) {
    console.log("values get trap");
    return target[prop] + 10;
  }
};
const proxyValues = new Proxy(targetValues, handlerValues);
console.log("values:", JSON.stringify(Object.values(proxyValues)));

console.log("\n=== Object.getOwnPropertySymbols on proxy ===");
const sym1 = Symbol("s1");
const sym2 = Symbol("s2");
const targetSymbols = {};
targetSymbols[sym1] = 1;
targetSymbols[sym2] = 2;
const proxySymbols = new Proxy(targetSymbols, {
  ownKeys(target) {
    console.log("symbols ownKeys trap");
    return [sym2];
  }
});
const symbols = Object.getOwnPropertySymbols(proxySymbols);
console.log("symbols length:", symbols.length);
console.log("symbols match:", symbols[0] === sym2);

console.log("\n=== Object.getOwnPropertyNames on proxy ===");
const target4 = { a: 1, b: 2 };
Object.defineProperty(target4, "hidden", { value: 3, enumerable: false, configurable: true });
const proxy4 = new Proxy(target4, {});
console.log("own names:", JSON.stringify(Object.getOwnPropertyNames(proxy4)));
