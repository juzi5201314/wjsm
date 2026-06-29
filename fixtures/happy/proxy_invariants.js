// Proxy invariant enforcement (ECMAScript §10.5 + §28.2) — spec-correct behavior.
//
// All violations below must throw a *catchable* TypeError. Tests use forms whose
// exceptions propagate synchronously (var-declarator init, statement-level call),
// which the engine routes through the IsException fork to the enclosing try/catch.

function expectThrow(fn, msg) {
    try {
        fn();
        console.log("FAIL:", msg, "(did not throw)");
    } catch (e) {
        console.log("PASS:", msg);
    }
}

// ── Constructor argument validation (ProxyCreate): target & handler must be objects ──
expectThrow(() => { let p = new Proxy(null, {}); }, "Proxy target null throws");
expectThrow(() => { let p = new Proxy("string", {}); }, "Proxy target string throws");
expectThrow(() => { let p = new Proxy({}, null); }, "Proxy handler null throws");
expectThrow(() => { let p = new Proxy({}, 42); }, "Proxy handler number throws");

// ── Revoked proxy: every internal method throws ──
(function testRevoked() {
    let { proxy, revoke } = Proxy.revocable({ x: 1 }, {});
    revoke();
    expectThrow(() => { let v = proxy.foo; }, "get on revoked proxy throws");
    expectThrow(() => { let v = ("foo" in proxy); }, "has on revoked proxy throws");
    expectThrow(() => Reflect.set(proxy, "foo", 1), "set on revoked proxy throws");
    expectThrow(() => Reflect.deleteProperty(proxy, "foo"), "delete on revoked proxy throws");
    expectThrow(() => Reflect.get(proxy, "foo"), "Reflect.get on revoked proxy throws");
})();

// ── [[Construct]] invariant: construct trap must return an object ──
(function testConstructInvariant() {
    let target = function () {};
    let nonObjectProxy = new Proxy(target, {
        construct(t, args, nt) { return 42; }
    });
    expectThrow(() => Reflect.construct(nonObjectProxy, []), "construct trap returning number throws");

    let stringProxy = new Proxy(target, {
        construct(t, args, nt) { return "not an object"; }
    });
    expectThrow(() => Reflect.construct(stringProxy, []), "construct trap returning string throws");

    // A construct trap returning an object is honoured.
    let okProxy = new Proxy(target, {
        construct(t, args, nt) { return { built: true }; }
    });
    let built = Reflect.construct(okProxy, []);
    console.log(built.built === true ? "PASS: construct trap object honoured"
                                     : "FAIL: construct trap object honoured");
})();

// ── apply trap forwards correctly ──
(function testApplyTrap() {
    let target = function () { return 42; };
    let proxy = new Proxy(target, {
        apply(t, thisArg, args) { return 99; }
    });
    console.log(proxy() === 99 ? "PASS: Proxy apply trap works"
                               : "FAIL: Proxy apply trap works");
})();

// ── GetMethod 读取 well-known symbol 必须通过 Proxy [[Get]] ──
(function testGetMethodUsesProxyGetTrap() {
    let target = function () {};
    let proxy = new Proxy(target, {
        get(t, prop, receiver) {
            if (prop === Symbol.hasInstance) {
                console.log("PASS: GetMethod proxy get trap called");
                return function (value) { return true; };
            }
            return t[prop];
        }
    });
    console.log({} instanceof proxy ? "PASS: proxy Symbol.hasInstance honoured"
                                  : "FAIL: proxy Symbol.hasInstance honoured");
})();

// ── Proxy.revocable returns a usable proxy of type "object" ──
(function testRevocableShape() {
    let { proxy, revoke } = Proxy.revocable({ a: 1 }, {});
    console.log(typeof proxy === "object" ? "PASS: revocable proxy is object"
                                          : "FAIL: revocable proxy is object");
    console.log(proxy.a === 1 ? "PASS: revocable proxy forwards get"
                              : "FAIL: revocable proxy forwards get");
    revoke();
    revoke(); // idempotent — second revoke is a no-op, must not throw
    console.log("PASS: revoke is idempotent");
})();

console.log("Proxy invariant tests completed");
