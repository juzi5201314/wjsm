const target = {};
const handler = {
  deleteProperty: function(t, p) {
    return true;
  }
};
const proxy = new Proxy(target, handler);
console.log(delete proxy.x);
