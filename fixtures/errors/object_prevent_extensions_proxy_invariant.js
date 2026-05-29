const proxy = new Proxy({}, {
    preventExtensions() {
        console.log("preventExtensions trap called");
        return true;
    }
});

Object.preventExtensions(proxy);
