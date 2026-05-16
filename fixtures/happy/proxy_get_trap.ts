const target = { x: 10 };
const handler = {
  get: function(t, p, r) {
    return 42;
  }
};
const proxy = new Proxy(target, handler);
console.log(proxy.x);
