var m = new Map(new Set([[1, 2], [3, 4]]));
console.log(m.size);
console.log(m.get(1));
console.log(m.get(3));

var s = new Set(new Set([5, 6, 5]));
console.log(s.size);
console.log(s.has(5));
console.log(s.has(6));

var custom = {};
custom[Symbol.iterator] = function() {
  var i = 0;
  return {
    next: function() {
      i = i + 1;
      if (i === 1) return { value: ["a", 7], done: false };
      if (i === 2) return { value: ["b", 8], done: false };
      return { value: undefined, done: true };
    }
  };
};
var fromCustom = new Map(custom);
console.log(fromCustom.size);
console.log(fromCustom.get("a"));
console.log(fromCustom.get("b"));
