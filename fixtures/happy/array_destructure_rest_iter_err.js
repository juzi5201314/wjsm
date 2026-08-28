// rest 收集循环的迭代器错误必须传播（IteratorStepValue 的 abrupt
// completion：next() 调用、done/value 读取抛出都要向外抛而不是死循环），
// 且 rest 耗尽迭代器后不得调用 return()（IteratorClose 只在
// [[Done]] 为 false 时发生）。
function poisonedValue() {
  return {
    [Symbol.iterator]() {
      return {
        next() {
          return {
            done: false,
            get value() {
              throw new Error("poisoned value");
            },
          };
        },
      };
    },
  };
}

function poisonedNext() {
  return {
    [Symbol.iterator]() {
      return {
        next() {
          throw new Error("poisoned next");
        },
      };
    },
  };
}

try {
  const [...x] = poisonedValue();
  console.log("unreachable", x);
} catch (e) {
  console.log("rest value err:", e.message);
}

try {
  const [...x] = poisonedNext();
  console.log("unreachable", x);
} catch (e) {
  console.log("rest step err:", e.message);
}

// 空位（elision）消耗迭代也要传播 next 错误。
try {
  const [, ...x] = poisonedNext();
  console.log("unreachable", x);
} catch (e) {
  console.log("elision step err:", e.message);
}

// 普通元素的 IteratorStepValue 错误同样传播。
try {
  const [x] = poisonedNext();
  console.log("unreachable", x);
} catch (e) {
  console.log("elem step err:", e.message);
}

// rest 收集结束于 done=true：不得再调用 return()。
let nextCount = 0;
let returnCount = 0;
const counting = {
  [Symbol.iterator]() {
    return {
      next() {
        nextCount += 1;
        return { done: true };
      },
      return() {
        returnCount += 1;
        return {};
      },
    };
  },
};
const [...collected] = counting;
console.log("collected:", collected.length, "next:", nextCount, "return:", returnCount);
