const target = { x: 10 };
const handler = {};
const proxy = new Proxy(target, handler);
console.log(proxy !== null);
console.log(target.x);
