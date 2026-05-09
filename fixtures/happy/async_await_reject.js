async function foo() {
  let v = await Promise.reject("rejected");
  console.log("should not reach");
}
foo().catch(function(e) {
  console.log(e);
});
