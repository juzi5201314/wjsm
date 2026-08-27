// 多个分支各自以尾调用结束：每个尾调用点都要独立改写成回边。
function walk(n, evens, odds) {
  if (n === 0) {
    return evens + ":" + odds;
  }
  if (n % 2 === 0) {
    return walk(n - 1, evens + 1, odds);
  }
  return walk(n - 1, evens, odds + 1);
}

console.log(walk(7, 0, 0));

// switch 分支尾调用 + 提前 return 混合。
function classify(n, seen) {
  switch (n) {
    case 0:
      return seen;
    case 1:
      return classify(n - 1, seen + "one,");
    default:
      return classify(n - 1, seen + n + ",");
  }
}

console.log(classify(4, ""));

// 循环体内的尾调用：回边目标仍是函数入口，循环状态每轮重建。
function collect(n, out) {
  if (n === 0) {
    return out;
  }
  let local = "";
  for (let i = 0; i < n; i = i + 1) {
    local = local + "*";
  }
  return collect(n - 1, out + local + "|");
}

console.log(collect(3, ""));
