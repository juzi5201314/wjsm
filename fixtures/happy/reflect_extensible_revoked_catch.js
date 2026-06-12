// Test Reflect.isExtensible / preventExtensions on revoked proxy
// Report claimed these might not surface exceptions correctly, but IR signature
// is (i64)->(i64) same as Reflect.get/deleteProperty which are proven catchable.

function testIsExtensibleRevoked() {
    let { proxy, revoke } = Proxy.revocable({}, {});
    revoke();

    try {
        Reflect.isExtensible(proxy);
        console.log("FAIL: isExtensible did not throw");
    } catch (e) {
        console.log("PASS: isExtensible caught", e.name);
    }
}

function testPreventExtensionsRevoked() {
    let { proxy, revoke } = Proxy.revocable({}, {});
    revoke();

    try {
        Reflect.preventExtensions(proxy);
        console.log("FAIL: preventExtensions did not throw");
    } catch (e) {
        console.log("PASS: preventExtensions caught", e.name);
    }
}

testIsExtensibleRevoked();
testPreventExtensionsRevoked();
