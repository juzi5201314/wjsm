var obj = {};

Object.defineProperty(obj, "x", {
  get: function() {
    return 7;
  },
  set: function(value) {
    this.y = value;
  },
});

console.log(obj.x);
obj.x = 9;
console.log(obj.y);
