function outer() {
  var x = 1;

  function mid() {
    function inner() {
      return x;
    }

    return inner;
  }

  var readX = mid();
  x = 3;
  return readX();
}

console.log(outer());
