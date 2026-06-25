try {
  for (const x of {
    [Symbol.iterator]() {
      return {
        next() { return { value: 1, done: false }; },
        return() { return undefined; },
      };
    },
  }) {
    break;
  }
} catch (e) {
  console.log(e.name);
}