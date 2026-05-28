// Test ownKeys trap
console.log("=== ownKeys filtering ===");
const target1 = { a: 1, b: 2, c: 3, d: 4 };
const handler1 = {
  ownKeys(target) {
    console.log("ownKeys trap called");
    return ["a", "c"];
  },
  getOwnPropertyDescriptor(target, prop) {
    return Object.getOwnPropertyDescriptor(target, prop);
  }
};
const proxy1 = new Proxy(target1, handler1);
console.log("keys:", JSON.stringify(Reflect.ownKeys(proxy1)));

console.log("\n=== ownKeys with non-configurable properties ===");
const target3 = {};
Object.defineProperty(target3, "fixed", {
  value: 42,
  writable: false,
  enumerable: true,
  configurable: false
});
target3.normal = "ok";

const handler3 = {
  ownKeys(target) {
    console.log("ownKeys trap called");
    return ["fixed", "normal"];
  },
  getOwnPropertyDescriptor(target, prop) {
    return Object.getOwnPropertyDescriptor(target, prop);
  }
};
const proxy3 = new Proxy(target3, handler3);
console.log("keys:", JSON.stringify(Reflect.ownKeys(proxy3)));
