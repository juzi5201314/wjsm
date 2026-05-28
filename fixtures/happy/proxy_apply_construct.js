// Test Proxy apply and construct traps
console.log("=== Proxy Apply Trap ===");

function add(a, b) {
  return a + b;
}

const applyHandler = {
  apply(target, thisArg, argumentsList) {
    console.log("apply trap called");
    console.log("args count:", argumentsList.length);
    return Reflect.apply(target, thisArg, argumentsList) * 2;
  }
};

const proxyAdd = new Proxy(add, applyHandler);
console.log("result:", proxyAdd(3, 4));

console.log("\n=== Proxy Construct Trap ===");

function Person(name, age) {
  this.name = name;
  this.age = age;
}

const constructHandler = {
  construct(target, argumentsList, newTarget) {
    console.log("construct trap called");
    console.log("args:", JSON.stringify(argumentsList));
    console.log("newTarget === target:", newTarget === target);
    const instance = Reflect.construct(target, argumentsList);
    instance.constructed = true;
    return instance;
  }
};

const ProxyPerson = new Proxy(Person, constructHandler);
const p = new ProxyPerson("Alice", 30);
console.log("name:", p.name);
console.log("age:", p.age);
console.log("constructed:", p.constructed);

console.log("\n=== Proxy without traps (forwarding) ===");
const forwardProxy = new Proxy(add, {});
console.log("forward result:", forwardProxy(10, 20));
