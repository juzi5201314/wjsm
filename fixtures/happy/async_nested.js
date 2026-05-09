async function inner() {
  return 10;
}

async function outer() {
  const v = await inner();
  console.log(v + 5);
}

outer();
