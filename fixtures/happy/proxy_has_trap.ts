const target = { x: 10, y: 20 };
const handler = {
  has: function(t, p) {
    return p === "x" || p === "z";
  }
};
const proxy = new Proxy(target, handler);
console.log("x" in proxy);
console.log("y" in proxy);
console.log("z" in proxy);
