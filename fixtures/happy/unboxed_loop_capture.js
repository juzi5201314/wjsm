// 被闭包捕获的变量不是帧局部，不得提升成浮点寄存器；
// 与之混用的循环仍要给出正确结果。
function makeCounter() {
  let count = 0;
  return {
    bump(times) {
      for (let i = 0; i < times; i++) {
        count += i;
      }
      return count;
    },
    read() {
      return count;
    },
  };
}

const counter = makeCounter();
console.log(counter.read());
console.log(counter.bump(4));
console.log(counter.read());
console.log(counter.bump(3));
console.log(typeof counter.read());

// 同一个局部混入非 number 写入时整体退回 boxed 表示。
function mixed(n) {
  let acc = 0;
  for (let i = 0; i < n; i++) {
    acc = i === 2 ? "two" : acc + i;
  }
  return acc;
}
console.log(mixed(2), typeof mixed(2));
console.log(mixed(4), typeof mixed(4));

// 循环里创建的闭包各自捕获自己的 let 绑定。
function closuresPerIteration(n) {
  const fns = [];
  for (let i = 0; i < n; i++) {
    fns.push(() => i * 2);
  }
  return fns.map((f) => f()).join(",");
}
console.log(closuresPerIteration(4));

// 捕获的累加器与未捕获的归纳变量并存。
function partialCapture(n) {
  let captured = 0;
  let plain = 0;
  const flush = () => captured;
  for (let i = 0; i < n; i++) {
    plain += i;
    captured += i * 2;
  }
  return [plain, flush()].join("/");
}
console.log(partialCapture(5));
