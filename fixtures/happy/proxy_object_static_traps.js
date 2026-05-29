// Object static methods must dispatch through Proxy internal methods.

const proto = { marker: "proto" };
const target = Object.create(proto);
const handler = {
    getPrototypeOf(target) {
        console.log("getPrototypeOf trap");
        return Reflect.getPrototypeOf(target);
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

const proxy = new Proxy(target, handler);

console.log("proto:", Object.getPrototypeOf(proxy) === proto);
console.log("extensible before:", Object.isExtensible(proxy));
console.log("prevent result same proxy:", Object.preventExtensions(proxy) === proxy);
console.log("extensible after:", Object.isExtensible(proxy));
