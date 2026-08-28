// 未捕获的 rest 收集 value 读取错误必须终止执行并以运行时错误退出，
// 而不是把异常值当普通值收集导致死循环。
const poisoned = {
  [Symbol.iterator]() {
    return {
      next() {
        return {
          done: false,
          get value() {
            throw new Error("poisoned rest value");
          },
        };
      },
    };
  },
};
const [...x] = poisoned;
console.log("unreachable", x.length);
