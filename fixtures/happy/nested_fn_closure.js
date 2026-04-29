function outer() {
  var y = 42;
  function inner() {
    return 100;
  }
  return inner() + y;
}
console.log(outer());
