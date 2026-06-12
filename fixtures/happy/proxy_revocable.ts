// Proxy.revocable: the proxy works until revoked; afterwards any internal method
// throws a catchable TypeError (ES §28.2.1.1 / §10.5). The revoked access is read
// into a variable so the exception propagates synchronously to the try/catch.
const target = { x: 10 };
const handler = {};
const { proxy, revoke } = Proxy.revocable(target, handler);
console.log(proxy.x); // 10 — forwards to target while live
revoke();
try {
  const v = proxy.x; // revoked → throws TypeError
  console.log("FAIL: revoked get did not throw", v);
} catch (e) {
  console.log("revoked-get-throws");
}
