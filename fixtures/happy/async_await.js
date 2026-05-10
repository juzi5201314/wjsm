async function greet() {
  const result = await Promise.resolve("hello");
  console.log(result);
}

greet();
