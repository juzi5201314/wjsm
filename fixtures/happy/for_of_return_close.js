let closed = false;
let iter = {
  [Symbol.iterator]() {
    return this;
  },
  next: () => ({ value: 1, done: false }),
  return: () => {
    closed = true;
    return { done: true };
  },
};

function run() {
  for (let value of iter) {
    console.log(value);
    return "done";
  }
}

console.log(run());
console.log(closed);
