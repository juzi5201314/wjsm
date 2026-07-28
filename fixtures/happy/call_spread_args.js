function collect() {
  return JSON.stringify([arguments.length, arguments[0], arguments[1], arguments[2]]);
}

console.log("plain", collect(...[1, 2]));
console.log("arrow", ((a, b) => a + b)(...[3, 4]));

const receiver = {
  base: 10,
  add(a, b) {
    return this.base + a + b;
  }
};
console.log("method", receiver.add(...[1, 2]));

class Pair {
  constructor(a, b) {
    this.value = a + b;
  }
}
console.log("new", new Pair(...[5, 6]).value);
console.log("builtin", Math.max(...[1, 5, 3]));

console.log("mixed", collect(0, ...[1, 2]));
console.log("multi", collect(...[1], ...[2], 3));
console.log("tail", collect(...[7, 8], 9));
console.log("string", collect(..."ab"));
console.log("set", collect(...new Set([4, 5])));

function* values() {
  yield 6;
  yield 7;
}
console.log("generator", collect(...values()));

const iterable = {
  [Symbol.iterator]() {
    let value = 8;
    return {
      next() {
        if (value > 9) return { done: true };
        return { value: value++, done: false };
      }
    };
  }
};
console.log("custom", collect(...iterable));

const overridden = [1, 2];
overridden[Symbol.iterator] = function* () {
  yield 10;
  yield 11;
};
console.log("override", collect(...overridden));

class Base {
  constructor(a, b) {
    this.sum = a + b;
  }
}
class Derived extends Base {
  constructor(...args) {
    super(...args);
  }
}
console.log("super", new Derived(12, 13).sum);
