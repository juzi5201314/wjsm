async function multi() {
  const a = await Promise.resolve(1);
  const b = await Promise.resolve(2);
  const c = await Promise.resolve(3);
  console.log(a + b + c);
}

multi();
