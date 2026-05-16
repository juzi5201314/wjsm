const target = { x: 10 };
const handler = {
  set: function(t, p, v, r) {
    t.x = v;
    return true;
  }
};
const proxy = new Proxy(target, handler);
proxy.x = 5;
console.log(target.x);
