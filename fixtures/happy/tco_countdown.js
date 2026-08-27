// 自递归尾调用被 loopification 改写为回边循环，结果必须与普通递归一致。
function countdown(n) {
  if (n === 0) {
    return "liftoff";
  }
  return countdown(n - 1);
}

console.log(countdown(0));
console.log(countdown(1));
console.log(countdown(10));

// 累加器形式：形参回写顺序必须与实参求值顺序无关。
function sum(n, acc) {
  if (n === 0) {
    return acc;
  }
  return sum(n - 1, acc + n);
}

console.log(sum(10, 0));

// 尾调用不是唯一出口：非尾位置的递归仍然走真实调用。
function factorial(n) {
  if (n <= 1) {
    return 1;
  }
  return n * factorial(n - 1);
}

console.log(factorial(6));
