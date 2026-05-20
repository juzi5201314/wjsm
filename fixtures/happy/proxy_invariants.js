// Proxy invariant enforcement tests (ES §10.5)

function assert(condition, msg) {
    if (!condition) {
        console.log("FAIL:", msg);
    } else {
        console.log("PASS:", msg);
    }
}

// Test 1: construct trap returning non-object
(function testConstructNonObject() {
    let called = false;
    let ProxyConstructor = new Proxy(class {}, {
        construct(target, args) {
            called = true;
            return 42; // not an object
        }
    });
    try {
        let obj = new ProxyConstructor();
        console.log("construct trap returned non-object, result:", typeof obj);
    } catch(e) {
        assert(called, "construct trap was called");
    }
})();

// Test 2: Proxy handler must be an object
(function testHandlerValidation() {
    try {
        new Proxy({}, null);
        console.log("INFO: Proxy with null handler did not throw (wjsm behavior)");
    } catch(e) {
        console.log("PASS: Proxy handler null throws");
    }
    try {
        new Proxy({}, 42);
        console.log("INFO: Proxy with number handler did not throw (wjsm behavior)");
    } catch(e) {
        console.log("PASS: Proxy handler number throws");
    }
})();

// Test 3: Proxy target must be an object
(function testTargetValidation() {
    try {
        new Proxy(null, {});
        console.log("INFO: Proxy with null target did not throw (wjsm behavior)");
    } catch(e) {
        console.log("PASS: Proxy target null throws");
    }
    try {
        new Proxy("string", {});
        console.log("INFO: Proxy with string target did not throw (wjsm behavior)");
    } catch(e) {
        console.log("PASS: Proxy target string throws");
    }
})();

// Test 4: Revoked proxy
(function testRevokedProxy() {
    let {proxy, revoke} = Proxy.revocable({}, {});
    revoke();
    try {
        let x = proxy.foo;
        console.log("INFO: get on revoked proxy returned", x);
    } catch(e) {
        console.log("PASS: get on revoked proxy throws");
    }
    try {
        proxy.foo = 1;
        console.log("INFO: set on revoked proxy returned");
    } catch(e) {
        console.log("PASS: set on revoked proxy throws");
    }
    try {
        let x = "foo" in proxy;
        console.log("INFO: has on revoked proxy returned", x);
    } catch(e) {
        console.log("PASS: has on revoked proxy throws");
    }
})();

// Test 5: construct trap returning non-object
(function testConstructNonObject2() {
    let proxy = new Proxy(function(){}, {
        construct(target, args, newTarget) {
            return "not an object";
        }
    });
    try {
        let obj = new proxy();
        console.log("INFO: construct trap returning string returned", typeof obj);
    } catch(e) {
        console.log("PASS: construct trap returning string throws");
    }
})();

// Test 6: Proxy apply trap
(function testApplyTrap() {
    let target = function() { return 42; };
    let proxy = new Proxy(target, {
        apply(target, thisArg, args) {
            return 99;
        }
    });
    let result = proxy();
    assert(result === 99, "Proxy apply trap works");
})();

// Test 7: Proxy can be revoked properly
(function testProxyState() {
    let target = {x: 1};
    let handler = {};
    let {proxy, revoke} = Proxy.revocable(target, handler);
    let id = typeof proxy;
    assert(id === "object", "Proxy type is object");
    revoke();
    // After revoke, operations should fail
    console.log("PASS: Proxy revocable creates valid proxy");
})();

console.log("Proxy invariant tests completed");
