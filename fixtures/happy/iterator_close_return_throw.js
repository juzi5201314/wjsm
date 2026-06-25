// ES §7.4.6: IteratorClose 时 return 抛错应替换原 completion
let caught = "";
try {
  for (const x of {
    [Symbol.iterator]() {
      let step = 0;
      return {
        next() {
          if (step++ === 0) return { value: 1, done: false };
          return { value: undefined, done: true };
        },
        return() {
          throw new Error("close-fail");
        },
      };
    },
  }) {
    throw new Error("body-fail");
  }
} catch (e) {
  caught = e.message;
}
console.log(caught);