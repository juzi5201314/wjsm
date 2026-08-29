function add(left, right) {
  return left + right;
}
let total = 0;
for (let i = 0; i < 120; i++) {
  total += add(1, 1);
}
console.log(total);
