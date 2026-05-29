const target = {};
Object.defineProperty(target, "x", {
    value: 1,
    configurable: true,
    enumerable: true,
    writable: true
});
Object.preventExtensions(target);

const proxy = new Proxy(target, {
    getOwnPropertyDescriptor(target, prop) {
        return undefined;
    }
});

Reflect.getOwnPropertyDescriptor(proxy, "x");
