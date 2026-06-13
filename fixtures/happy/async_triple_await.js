async function triple() {
  await Promise.resolve(1);
  console.log("a");
  await Promise.resolve(2);
  console.log("b");
  await Promise.resolve(3);
  console.log("c");
}

triple();