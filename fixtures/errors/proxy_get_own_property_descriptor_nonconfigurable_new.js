const proxy = new Proxy({}, {
    getOwnPropertyDescriptor(target, prop) {
        return { value: 1, configurable: false };
    }
});

Reflect.getOwnPropertyDescriptor(proxy, "x");
