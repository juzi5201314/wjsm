const target = { x: 10, y: 20 };
const proxy = new Proxy(target, {});
console.log(Reflect.get(proxy, "x", proxy));
