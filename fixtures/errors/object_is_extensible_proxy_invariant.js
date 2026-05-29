const target = {};
Object.preventExtensions(target);

const proxy = new Proxy(target, {
    isExtensible() {
        console.log("isExtensible trap called");
        return true;
    }
});

Object.isExtensible(proxy);
