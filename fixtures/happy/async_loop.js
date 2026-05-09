async function delay(v) {
  return v;
}
async function foo() {
  let sum = 0;
  for (let i = 1; i <= 3; i++) {
    sum += await delay(i);
  }
  console.log(sum);
}
foo();
