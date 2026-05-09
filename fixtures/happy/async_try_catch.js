async function rejector() {
  return Promise.reject("caught");
}
async function foo() {
  try {
    let v = await rejector();
    console.log("should not reach");
  } catch(e) {
    console.log(e);
  }
}
foo();
