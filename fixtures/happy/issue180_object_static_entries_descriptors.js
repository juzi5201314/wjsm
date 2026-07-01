// Issue #180: Object.fromEntries, getOwnPropertyDescriptors, defineProperties
const from = Object.fromEntries([
  ["a", 1],
  ["b", 2],
]);
console.log("fromEntries a:", from.a);
console.log("fromEntries b:", from.b);

const src = { x: 10, y: 20 };
const descs = Object.getOwnPropertyDescriptors(src);
console.log("gopd x value:", descs.x.value);
console.log("gopd x enumerable:", descs.x.enumerable);
console.log("gopd keys:", Object.keys(descs).sort().join(","));

const target = {};
Object.defineProperties(target, {
  foo: { value: 42, writable: false, enumerable: true, configurable: true },
  bar: { value: 99, enumerable: false },
});
console.log("defineProperties foo:", target.foo);
console.log("defineProperties bar:", target.bar);
console.log("defineProperties keys:", Object.keys(target).join(","));