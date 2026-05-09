async function foo() {
  throw "oops";
}
foo().catch(function(e) {
  console.log(e);
});
