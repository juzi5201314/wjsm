let closed = false;
let iter = {
  next: () => ({ value: 1, done: false }),
  return: () => {
    closed = true;
    return { done: true };
  },
};

try {
  for (let value of iter) {
    console.log(value);
    throw "boom";
  }
} catch (err) {
  console.log(err);
}

console.log(closed);
