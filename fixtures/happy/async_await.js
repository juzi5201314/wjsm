async function bar() {
  return 10;
}
async function foo() {
  let v = await bar();
  console.log(v + 1);
}
foo();
