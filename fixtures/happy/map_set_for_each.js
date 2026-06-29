var m = new Map();
m.set("a", 1);
m.set("b", 2);
m.forEach(function(value, key, self) {
  console.log(key);
  console.log(value);
  console.log(self === m);
});

var s = new Set();
s.add(3);
s.add(4);
s.forEach(function(value, key, self) {
  console.log(value);
  console.log(key);
  console.log(self === s);
});
