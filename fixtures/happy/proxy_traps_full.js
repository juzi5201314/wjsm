// 1. get / set / has / deleteProperty / getOwnPropertyDescriptor / defineProperty
console.log("=== Basic Traps ===");
const target1 = { a: 1 };
const handler1 = {
    get(target, prop, receiver) {
        console.log("get trap:", prop);
        return Reflect.get(target, prop, receiver) + 10;
    },
    set(target, prop, value, receiver) {
        console.log("set trap:", prop, "to", value);
        return Reflect.set(target, prop, value + 5, receiver);
    },
    has(target, prop) {
        console.log("has trap:", prop);
        return Reflect.has(target, prop);
    },
    deleteProperty(target, prop) {
        console.log("deleteProperty trap:", prop);
        return Reflect.deleteProperty(target, prop);
    },
    getOwnPropertyDescriptor(target, prop) {
        console.log("getOwnPropertyDescriptor trap:", prop);
        return Reflect.getOwnPropertyDescriptor(target, prop);
    },
    defineProperty(target, prop, descriptor) {
        console.log("defineProperty trap:", prop);
        return Reflect.defineProperty(target, prop, descriptor);
    }
};

const proxy1 = new Proxy(target1, handler1);
console.log("get:", proxy1.a);
proxy1.a = 100;
console.log("after set target:", target1.a);
console.log("after set proxy:", proxy1.a);
console.log("has:", "a" in proxy1);
delete proxy1.a;
console.log("after delete target:", target1.a);

Reflect.defineProperty(proxy1, "b", { value: 42, configurable: true, enumerable: true, writable: true });
console.log("after defineProperty target:", target1.b);
console.log("descriptor:", JSON.stringify(Reflect.getOwnPropertyDescriptor(proxy1, "b")));

// 2. getPrototypeOf / setPrototypeOf / isExtensible / preventExtensions
console.log("=== Prototype & Extensibility Traps ===");
const targetProto = {};
const target2 = Object.create(targetProto);
const handler2 = {
    getPrototypeOf(target) {
        console.log("getPrototypeOf trap");
        return Reflect.getPrototypeOf(target);
    },
    setPrototypeOf(target, proto) {
        console.log("setPrototypeOf trap");
        return Reflect.setPrototypeOf(target, proto);
    },
    isExtensible(target) {
        console.log("isExtensible trap");
        return Reflect.isExtensible(target);
    },
    preventExtensions(target) {
        console.log("preventExtensions trap");
        return Reflect.preventExtensions(target);
    }
};
const proxy2 = new Proxy(target2, handler2);
console.log("getPrototypeOf:", Reflect.getPrototypeOf(proxy2) === targetProto);

const newProto = { protoField: "ok" };
console.log("setPrototypeOf:", Reflect.setPrototypeOf(proxy2, newProto));
console.log("after setPrototypeOf:", Reflect.getPrototypeOf(proxy2).protoField);

console.log("isExtensible:", Reflect.isExtensible(proxy2));
console.log("preventExtensions:", Reflect.preventExtensions(proxy2));
console.log("after preventExtensions isExtensible:", Reflect.isExtensible(proxy2));

// 3. ownKeys
console.log("=== ownKeys Trap ===");
const target3 = { x: 1, y: 2 };
const handler3 = {
    ownKeys(target) {
        console.log("ownKeys trap");
        return ["y", "z"];
    }
};
const proxy3 = new Proxy(target3, handler3);
console.log("ownKeys:", JSON.stringify(Reflect.ownKeys(proxy3)));

// 4. apply / construct
console.log("=== apply & construct Traps ===");
function sum(a, b) {
    return a + b;
}
const handler4 = {
    apply(target, thisArg, argumentsList) {
        console.log("apply trap:", JSON.stringify(argumentsList));
        return Reflect.apply(target, thisArg, argumentsList) * 2;
    },
    construct(target, argumentsList, newTarget) {
        console.log("construct trap:", JSON.stringify(argumentsList));
        const instance = Reflect.construct(target, argumentsList, newTarget);
        instance.constructed = true;
        return instance;
    }
};
const proxy4 = new Proxy(sum, handler4);
console.log("apply call:", proxy4(5, 6));

function Person(name) {
    this.name = name;
}
const proxyPerson = new Proxy(Person, handler4);
const instance = new proxyPerson("Alice");
console.log("constructed name:", instance.name);
console.log("constructed flag:", instance.constructed);
console.log("instanceof proxyPerson:", instance instanceof proxyPerson);
console.log("instanceof Person:", instance instanceof Person);
