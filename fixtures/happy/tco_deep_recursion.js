function count(n, acc) {
  if (n === 0) return acc;
  return count(n - 1, acc + 1);
}
console.log(count(200000, 0));
