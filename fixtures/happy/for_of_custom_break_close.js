let closed = false;
let iter = {
  next: () => ({ value: 1, done: false }),
  return: () => {
    closed = true;
    return { done: true };
  },
};

for (let value of iter) {
  console.log(value);
  break;
}

console.log(closed);
