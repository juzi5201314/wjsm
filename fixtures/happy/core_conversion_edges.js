console.log(typeof null);
console.log(typeof (() => 0));
console.log(typeof /x/);
console.log(1 === 1);
console.log(NaN === NaN);
console.log("x" === "x");

const hinted = {};
hinted[Symbol.toPrimitive] = function (hint) {
  console.log(hint);
  return hint === "string" ? "hinted" : 12;
};
console.log(Number(hinted));
console.log(String(hinted));

const ordinary = {
  valueOf() {
    return 41;
  },
  toString() {
    return "ordinary";
  },
};
console.log(Number(ordinary));
console.log(String(ordinary));
console.log(Number(/x/));
console.log(String(/x/));

console.log(Object.prototype.toString.call(null));
console.log(Object.prototype.toString.call(undefined));
console.log(Object.prototype.toString.call(Object(true)));
console.log(Object.prototype.toString.call(Object(1)));
console.log(Object.prototype.toString.call(Object("x")));
console.log(Object.prototype.toString.call(Object(1n)));
console.log(Object.prototype.toString.call(Object(Symbol("x"))));
console.log(Object.prototype.toString.call([]));
console.log(Object.prototype.toString.call(() => 0));
console.log(Object.prototype.toString.call(/x/));
const typeError = new TypeError("x");
console.log(Object.prototype.toString.call(typeError));
const tagged = {};
tagged[Symbol.toStringTag] = "Custom";
console.log(Object.prototype.toString.call(tagged));
console.log(Object.prototype.toString.call(new Proxy([], {})));

const arrayLike = { "0": "x", length: 1, join: Array.prototype.join };
console.log(Array.prototype.toString.call(arrayLike));
const customJoin = {
  join() {
    return "custom";
  },
};
console.log(Array.prototype.toString.call(customJoin));
console.log(Array.prototype.toString.call({ join: 1 }));

const array = [];
Object.defineProperty(array, "named", {
  get() {
    return 7;
  },
  set(value) {
    this.written = value;
  },
  enumerable: true,
  configurable: true,
});
console.log(array.named);
array.named = 9;
console.log(array.written);
const namedDescriptor = Object.getOwnPropertyDescriptor(array, "named");
console.log(typeof namedDescriptor.get);
console.log(typeof namedDescriptor.set);
console.log(namedDescriptor.enumerable);
console.log(namedDescriptor.configurable);

console.log(Array.prototype.toString.name);
console.log(Array.prototype.toString.length);
const toStringDescriptor = Object.getOwnPropertyDescriptor(Array.prototype, "toString");
console.log(toStringDescriptor.writable);
console.log(toStringDescriptor.enumerable);
console.log(toStringDescriptor.configurable);
