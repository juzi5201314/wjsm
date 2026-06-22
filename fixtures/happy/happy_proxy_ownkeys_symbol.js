// Proxy ownKeys must include Symbol keys on non-extensible target (#186)
const sym = Symbol("s");
const target = { a: 1 };
target[sym] = 2;
Object.preventExtensions(target);

const proxy = new Proxy(target, {
    ownKeys() {
        return ["a"];
    }
});

try {
    Object.keys(proxy);
    console.log("FAIL: did not throw");
} catch (e) {
    console.log("PASS", e.name);
}