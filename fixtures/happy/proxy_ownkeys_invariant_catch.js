// Proxy ownKeys invariant violations should throw catchable TypeErrors.
// Non-extensible target: trap result must match target's own keys exactly.

function testNonExtensibleInvariant() {
    const target = { a: 1, b: 2 };
    Object.preventExtensions(target);

    const proxy = new Proxy(target, {
        ownKeys(t) {
            // Violates invariant: omits key 'b' when target is non-extensible
            return ['a'];
        }
    });

    try {
        Object.keys(proxy);
        console.log("FAIL: did not throw");
    } catch (e) {
        console.log("PASS: caught", e.name);
    }
}

function testNonConfigurableInvariant() {
    const target = {};
    Object.defineProperty(target, "fixed", {
        value: 42,
        configurable: false,
        enumerable: true
    });
    Object.defineProperty(target, "normal", {
        value: 99,
        configurable: true,
        enumerable: true
    });

    const proxy = new Proxy(target, {
        ownKeys(t) {
            // Violates invariant: omits non-configurable property 'fixed'
            return ['normal'];
        }
    });

    try {
        Object.getOwnPropertyNames(proxy);
        console.log("FAIL: did not throw for non-configurable");
    } catch (e) {
        console.log("PASS: caught non-configurable invariant", e.name);
    }
}

testNonExtensibleInvariant();
testNonConfigurableInvariant();
