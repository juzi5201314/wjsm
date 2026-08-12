function recurse(n) {
  return recurse(n + 1);
}
try {
  recurse(0);
} catch (error) {
  console.log(error instanceof RangeError);
  console.log(error.name);
  console.log(error.message);
  console.log(error.stack);
}
console.log("continued");
