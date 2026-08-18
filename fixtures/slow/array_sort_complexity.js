// Array.prototype.sort 的比较次数应保持 O(n log n)，避免退化为二次复杂度。
var input = Array.from({ length: 1000 }, (_, i) => (i * 7919) % 10007);
var comparisons = 0;
var desc = input.sort((x, y) => {
  comparisons++;
  return y - x;
});
console.log(desc[0], desc[999], comparisons <= 12000);
