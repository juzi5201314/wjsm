function outer() {
  function Inner() {}
  return Inner;
}

const F = outer();

Object.defineProperties(F.prototype, {
  x: {
    configurable: true,
    enumerable: true,
    get() {
      return 1;
    },
  },
  [Symbol.toStringTag]: {
    configurable: true,
    value: 'X',
  },
});

console.log(F.prototype.x === 1);
console.log(F.prototype[Symbol.toStringTag] === 'X');

const symbolDescriptor = Object.getOwnPropertyDescriptor(
  F.prototype,
  Symbol.toStringTag,
);
console.log(
  symbolDescriptor.value === 'X' &&
    symbolDescriptor.configurable === true &&
    symbolDescriptor.enumerable === false &&
    symbolDescriptor.writable === false,
);

const descriptor = Object.getOwnPropertyDescriptor(F.prototype, 'x');
console.log(
  typeof descriptor.get === 'function' &&
    descriptor.enumerable === true &&
    descriptor.configurable === true,
);
