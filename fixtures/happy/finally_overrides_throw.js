try {
  throw 1;
} finally {
  console.log("finally");
  return 9;
}
console.log("after");
