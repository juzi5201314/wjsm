// Test Reflect.get/set without receiver parameter (WASM stack padding)
const obj = { x: 10, y: 20 };

// Reflect.get without receiver
const x = Reflect.get(obj, 'x');
console.log(x);

// Reflect.set without receiver
const setResult = Reflect.set(obj, 'z', 30);
console.log(setResult);
console.log(obj.z);

// Test with proxy get trap (receiver omitted — triggers padding fix)
const handler1 = {
  get(target, prop) {
    return target[prop] * 2;
  }
};
const proxy1 = new Proxy(obj, handler1);
const doubled = Reflect.get(proxy1, 'x');
console.log(doubled);

// Test with proxy set trap (receiver omitted — triggers padding fix)
let setCalled = false;
const handler2 = {
  set(target, prop, value) {
    setCalled = true;
    target[prop] = value + 1;
    return true;
  }
};
const proxy2 = new Proxy(obj, handler2);
Reflect.set(proxy2, 'w', 100);
console.log(setCalled);
