const target = { x: 10 };
const handler = {};
const { proxy, revoke } = Proxy.revocable(target, handler);
console.log(proxy.x);
revoke();
console.log(proxy.x);
