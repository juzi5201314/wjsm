function outer() {
  var y = 42;
  function inner() {
    return y;
  }
  return inner();
}
console.log(outer());
