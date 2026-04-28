let total = 0;
outer: for (let i = 0; i < 5; i = i + 1) {
  if (i === 1) {
    continue outer;
  }
  if (i === 3) {
    break outer;
  }
  total = total + i;
}
console.log(total);
