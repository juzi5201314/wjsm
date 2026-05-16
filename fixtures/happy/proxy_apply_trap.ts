function target(a, b) {
  return a + b;
}
const handler = {
  apply: function(t, thisArg, args) {
    return args[0] * args[1];
  }
};
const proxy = new Proxy(target, handler);
console.log(proxy(3, 4));
