// 同一函数存在多个尾调用位置时，每个位置都要各自降为跳回入口。
function classify(n, evens, odds) {
  if (n === 0) return evens + ":" + odds;
  if (n % 2 === 0) {
    return classify(n - 1, evens + 1, odds);
  }
  return classify(n - 1, evens, odds + 1);
}
console.log(classify(10, 0, 0));

// 尾调用之后仍要走非尾路径的函数保持普通递归语义。
function sum(n) {
  if (n === 0) return 0;
  return n + sum(n - 1);
}
console.log(sum(10));
