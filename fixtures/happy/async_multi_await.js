async function multi() {
  const a = await Promise.resolve(10);
  const b = await Promise.resolve(20);
  console.log(a);
  console.log(b);
}

multi();
