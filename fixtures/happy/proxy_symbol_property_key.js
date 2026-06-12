const proxy = new Proxy({}, {
    get(target, key, receiver) {
        console.log(key === Symbol.iterator);
        console.log(typeof key);
        return 42;
    },
    set(target, key, value, receiver) {
        console.log(key === Symbol.iterator);
        return true;
    },
    has(target, key) {
        console.log(key === Symbol.iterator);
        return true;
    }
});
console.log(proxy[Symbol.iterator]);
console.log(Reflect.set(proxy, Symbol.iterator, 1));
console.log(Symbol.iterator in proxy);
