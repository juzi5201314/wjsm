function makeGetter(value) {
  return function() {
    return value;
  };
}

function makeSetter(target) {
  return function(value) {
    target.y = value + 1;
  };
}

var obj = {};
Object.defineProperty(obj, "x", { get: makeGetter(11) });
Object.defineProperty(obj, "z", { set: makeSetter(obj) });

console.log(obj.x);
obj.z = 4;
console.log(obj.y);
