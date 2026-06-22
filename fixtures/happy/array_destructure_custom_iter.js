const [m, n] = {
  [Symbol.iterator]() {
    let i = 0;
    return {
      next() {
        i++;
        return { value: i, done: i > 2 };
      },
    };
  },
};
console.log(m);
console.log(n);