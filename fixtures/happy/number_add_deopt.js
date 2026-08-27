function add1(x) {
  return x + 1;
}
let s = 0;
for (let i = 0; i < 120; i = i + 1) {
  s = s + add1(1);
}
s = s + add1("1");
console.log(s);
