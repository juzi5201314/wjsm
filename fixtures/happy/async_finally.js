async function foo() {
  try {
    await Promise.resolve(1);
    console.log("try");
  } finally {
    console.log("finally");
  }
}
foo();
