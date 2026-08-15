function choose(flag) {
  if (flag) {
    return 11;
  }
  return 22;
}

console.log(choose(true));
console.log(choose(false));

let point = { x: 0 };
for (let i = 0; i < 3; i = i + 1) {
  point.x = point.x + 1;
}
console.log(point.x);
