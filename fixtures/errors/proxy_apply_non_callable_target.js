const proxy = new Proxy({}, {
    apply() {
        console.log("apply trap called");
        return 1;
    }
});

proxy();
