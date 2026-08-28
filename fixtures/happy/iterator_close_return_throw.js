// ES §7.4.6 IteratorClose：completion 为 throw 时（步骤 5）原始异常胜出，
// return() 自身的抛错被吞咽；completion 非 throw（break，步骤 6）时
// return() 的抛错传播。
function make() {
  let step = 0;
  return {
    [Symbol.iterator]() {
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
  };
}

try {
  for (const x of make()) {
    throw new Error("body-fail");
  }
} catch (e) {
  console.log("throw-completion:", e.message);
}

try {
  for (const x of make()) {
    break;
  }
} catch (e) {
  console.log("break-completion:", e.message);
}
