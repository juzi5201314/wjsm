function create() {
  var a = 1;
  var b = 2;
  function inner() {
    return a + b;
  }
  return inner();
}
console.log(create());
