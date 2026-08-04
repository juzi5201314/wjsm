// Issue #357 regression: curry closure creation, shared mutable bindings,
// deep captures through the environment prototype chain, and retained closures.
function makeCounter(seed) {
  let value = seed;
  return {
    read: () => value,
    increment: () => {
      value += 1;
      return value;
    },
  };
}

function makeDeep(seed) {
  let outer = seed;
  return function middle() {
    return function inner() {
      outer += 2;
      return outer;
    };
  };
}

const add = (a) => (b) => a + b;
const counter = makeCounter(0);
const deep = makeDeep(1)();
const retained = [
  () => 0,
  () => 1,
  () => 2,
  () => 3,
];

console.log(add(1)(2));
console.log(counter.read(), counter.increment(), counter.read());
console.log(deep(), deep());
console.log(retained[0](), retained[3]());
