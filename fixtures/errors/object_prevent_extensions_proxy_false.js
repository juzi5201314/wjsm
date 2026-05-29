const proxy = new Proxy({}, {
    preventExtensions() {
        console.log("preventExtensions trap called");
        return false;
    }
});

Object.preventExtensions(proxy);
