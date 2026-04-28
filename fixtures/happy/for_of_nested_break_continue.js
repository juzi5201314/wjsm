let total = 0;
for (let ch of "abc") {
  total = total + 1;
  if (total === 1) {
    continue;
  }
  console.log(ch);
}
console.log(total);
