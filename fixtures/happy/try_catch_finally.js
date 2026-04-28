try {
  throw 42;
} catch (e) {
  console.log(e);
} finally {
  console.log("finally");
}
console.log("after");
